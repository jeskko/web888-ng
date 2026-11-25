# SPDX-License-Identifier: GPL-3.0-or-later
"""Training examples for the CW noise reduction (for copying by ear), at
the page's 12 kS/s, made on the GPU like data.py's:

* signals: the wanted station at a pitch of 300-1000 Hz and, on 60 %,
  1-3 others anywhere in the passband at -20..+10 dB; keying from the
  neural CW decoder's synthesiser (tools/ml/cw/synth.py: QSO text,
  10-45 wpm, hand-keying jitter, dash/dot ratio, Farnsworth gaps), edges
  of 1-8 ms, a slow drift; each faded on its own (ionochannel's Watterson
  tap gains): none, slow QSB (0.04-0.85 Hz spread) or, on 12 %, auroral
  flutter (3-30 Hz);
* noise: white or impulsive atmospheric (QRN, Vd 1-12 dB) at an SNR in
  2500 Hz against the wanted station's key-down power, or (half) real HF
  noise from the Paderborn pauses (noisebank.py); at times
  switching-supply hash, arcing or powerline bursts;
* receiver: a CW filter (200-500, 500-1000 Hz around the pitch) or an
  SSB one (2.4-3 kHz), the AGC on most (its gain from the sum, on both
  paths).

Every CW signal is something to hear, so the target is all of them
(faded, filtered, the AGC's gain) and, `residual` dB below, the noise:
the network learns to take away noise, not to pick a station. One
example in ten has no signal.
"""
from __future__ import annotations

import importlib.util
import os
import random
import sys

import numpy as np
import torch

from common import FS
from data import N, NoiseBank, active_power_t, ic

TAU = 2 * np.pi
OTHERS = 3
QSB = (0.04, 0.85)
FLUTTER = (3.0, 30.0)


def _synth():
    """tools/ml/cw/synth.py (its own `common` is the CW decoder's)."""
    d = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../cw")

    def load(name, f):
        spec = importlib.util.spec_from_file_location(name, os.path.join(d, f))
        m = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(m)
        return m

    saved = sys.modules.get("common")
    sys.modules["common"] = load("cw_common", "common.py")
    try:
        return load("cw_synth", "synth.py")
    finally:
        if saved is not None:
            sys.modules["common"] = saved
        else:
            del sys.modules["common"]


SYN = _synth()


def key(r: random.Random, n: int = N) -> np.ndarray:
    """A station's key-down (0/1) over n samples at FS: a line of traffic
    (more after a pause if short), the window anywhere over it, often
    with silence before or after."""
    wpm = float(np.exp(r.uniform(np.log(10), np.log(45))))
    jitter = r.choice([0.0, 0.0, r.uniform(0, 0.08), r.uniform(0.05, 0.18)])
    dash = r.uniform(2.6, 3.8) if r.random() < 0.5 else 3.0
    fw = r.uniform(0.6, 1.0) * wpm if r.random() < 0.15 else wpm
    w = n / FS
    line = SYN.text(r)
    while True:
        marks, _, total = SYN.keying(line, r, wpm, jitter, dash, fw)
        if total >= w + 1 or r.random() < 0.2:
            break
        line = line + (" | " if r.random() < 0.7 else " ") + SYN.text(r)
    w0 = r.uniform(-1.0, max(-0.9, total - w + 1.0))
    k = np.zeros(n, np.float32)
    for m0, m1, _ in marks:
        i0, i1 = int(round((m0 - w0) * FS)), int(round((m1 - w0) * FS))
        if i1 > 0 and i0 < n:
            k[max(i0, 0):min(max(i1, i0 + 1), n)] = 1.0
    return k


def _gains(rows: int, spread: torch.Tensor, smax: float, gen) -> torch.Tensor:
    """(rows, N) Rayleigh gains, each row its own spread (<= smax)."""
    import ionochannel.torch as ict
    rate = (spread / smax).reshape(rows, 1).expand(rows, N).to(torch.float64)
    return ict.tap_gain(ic.scatter(0.0, 1.0, smax), N, FS, gen, rows, gen.device, rate=rate)


def batch(B: int, rng, gen, snr=(-18.0, 20.0), noise: NoiseBank | None = None, p_real: float = 0.5,
          residual=(10.0, 15.0), group: int = 8):
    """(inputs, targets) (B, N) float32 on gen's device."""
    import ionochannel.torch as ict
    from ionochannel.receiver import agc_block, agc_gains
    dev = gen.device
    U = lambda *s, a=0.0, b=1.0: a + (b - a) * torch.rand(*s, device=dev, generator=gen)  # noqa: E731
    r = random.Random(int(rng.integers(1 << 62)))
    M = 1 + OTHERS
    # keying: the wanted station, then the others (on 60 % of rows: 1-3)
    keys = np.zeros((B, M, N), np.float32)
    for b in range(B):
        if rng.random() < 0.1:
            continue                                            # noise only
        keys[b, 0] = key(r)
        if rng.random() < 0.6:
            for m in range(1, 1 + int(rng.choice([1, 1, 2, 3]))):
                keys[b, m] = key(r)
    K = torch.from_numpy(keys).to(dev).reshape(B * M, 1, N)
    # edges: raised cosine, 1-8 ms (per batch)
    m = max(1, int(rng.uniform(0.001, 0.008) * FS / 2))
    win = torch.hann_window(2 * m + 3, periodic=False, device=dev)
    K = torch.nn.functional.conv1d(K, (win / win.sum())[None, None, :], padding=m + 1)[:, 0, :N].reshape(B, M, N)
    # pitches and the passband: the wanted at 300-1000 Hz; a CW filter
    # around it (its centre within a quarter of its width) or SSB
    pitch = U(B, 1, a=300.0, b=1000.0)
    t = torch.arange(N, device=dev) / FS
    kind = rng.random(B // group + 1)
    lo, hi = torch.empty(B, 1, device=dev), torch.empty(B, 1, device=dev)
    bands = []
    for gi, r0 in enumerate(range(0, B, group)):
        rows = slice(r0, min(B, r0 + group))
        if kind[gi] < 0.4:
            w = rng.uniform(200, 500)
        elif kind[gi] < 0.7:
            w = rng.uniform(500, 1000)
        else:
            w = None
        if w is None:
            a, b = rng.uniform(150, 400), rng.uniform(2400, 3000)
            lo[rows], hi[rows] = a, b
            bands.append((a, b, 100.0))
        else:
            # one filter per group: the group's pitches moved under it
            c = rng.uniform(450, 850)
            pitch[rows] = c + (U(pitch[rows].shape[0], 1) - 0.5) * w / 2
            lo[rows], hi[rows] = c - w / 2, c + w / 2
            bands.append((c - w / 2, c + w / 2, min(100.0, w / 4)))
    # the others: anywhere from 50 Hz below the passband to 50 Hz above
    f = torch.cat([pitch, lo - 50 + (hi - lo + 100) * U(B, OTHERS)], dim=1)               # (B, M)
    lvl = torch.cat([torch.ones(B, 1, device=dev), 10 ** (U(B, OTHERS, a=-20.0, b=10.0) / 20)], dim=1)
    drift = U(B, M, 1, a=-3.0, b=3.0) * t[None, None, :] / (N / FS)                         # Hz
    ph = TAU * torch.cumsum((f[:, :, None] + drift) / FS, dim=-1) + U(B, M, 1, b=TAU)
    # fading per station: none, QSB on 40 %, flutter on 12 % of rows
    R = B * M
    qsb = _gains(R, U(R, a=QSB[0], b=QSB[1]), QSB[1], gen)
    fl = _gains(R, torch.exp(U(R, a=float(np.log(FLUTTER[0])), b=float(np.log(FLUTTER[1])))), FLUTTER[1], gen)
    sel = U(B, 1).expand(B, M).reshape(R, 1)
    fade = torch.where(sel < 0.12, fl, torch.where(U(R, 1) < 0.4, qsb, torch.ones_like(qsb)))
    zs = (lvl[:, :, None] * K * torch.exp(1j * ph) * fade.reshape(B, M, N)).sum(1).to(torch.complex64)
    # noise: in 2500 Hz, 10^(-snr/10) of the wanted's key-down power
    sn = U(B, 1, a=snr[0], b=snr[1])
    sd = torch.sqrt(10 ** (-sn / 10) * FS / 2500)
    zn = torch.zeros((B, N), dtype=torch.complex64, device=dev)
    for r0 in range(0, B, group):
        rows = min(group, B - r0)
        if noise is not None and rng.random() < p_real:
            # real noise (unit power in 300-2700 Hz): its analytic signal
            nz = torch.from_numpy(noise.clips(rng, rows, N)).to(dev) * (2500 / 2400) ** 0.5
            zn[r0:r0 + rows] = ict._analytic(nz.to(torch.complex64), ic.channel.fast_len(2 * N))[:, :N]
        else:
            vd = 1.05 if rng.random() < 0.5 else rng.uniform(1.05, 12.0)
            zn[r0:r0 + rows] = ict.qrn_noise((rows,), N, FS, vd, gen)
    zn = zn * sd

    def add(p, a, b, make):
        idx = np.flatnonzero(rng.random(B) < p)
        if len(idx):
            ix = torch.as_tensor(idx, device=dev)
            zn[ix] += (sd[ix] * 10 ** (U(len(idx), 1, a=a, b=b) / 20)).to(torch.complex64) * make(len(idx))

    spec = ic.Smps(mains_hz=float(rng.choice([50.0, 60.0])), am_depth=rng.uniform(0, 0.5),
                   fm_dev_hz=rng.uniform(0, 20), drift_hz=rng.uniform(0, 30))
    add(0.15, 0, 15, lambda k: ict.smps_noise((k,), N, FS, rng.uniform(150, 2900), gen, spec))
    add(0.05, 5, 20, lambda k: ict.arcing_noise((k,), N, FS, gen))
    add(0.05, 5, 15, lambda k: ict.bursts_noise((k,), N, FS, gen))
    # the receiver's filter, per group
    ys, yn = torch.empty_like(zs), torch.empty_like(zn)
    for gi, r0 in enumerate(range(0, B, group)):
        a, b, tr = bands[gi]
        rx = ict.Receiver(FS, (a, b), transition_hz=tr)
        ys[r0:r0 + group] = rx.apply(zs[r0:r0 + group])
        yn[r0:r0 + group] = rx.apply(zn[r0:r0 + group])
    # the AGC on 80 %: its gain from the sum, on both paths
    a = ic.Agc(decay_s=rng.uniform(0.1, 2.0), hang_s=float(rng.choice([0.0, 0.3])))
    bl = agc_block(FS)
    nb = -(-N // bl)
    env = torch.zeros((B, nb * bl), device=dev)
    env[:, :N] = (ys + yn).abs()
    peaks = env.reshape(B, nb, bl).amax(-1).double().cpu().numpy()
    gk = np.ones((B, nb))
    agc = rng.random(B) < 0.8
    for i in np.flatnonzero(agc):
        gk[i] = agc_gains(peaks[i], FS, a)
    gk = torch.as_tensor(gk, device=dev, dtype=torch.float32)
    prev = torch.cat([gk[:, :1], gk[:, :-1]], dim=-1)
    ramp = torch.arange(1, bl + 1, device=dev) / bl
    gain = (prev[..., None] + (gk - prev)[..., None] * ramp).reshape(B, nb * bl)[:, :N]
    # level: as the voice data's after its AGC; rows without the AGC at
    # their own level
    p = active_power_t((ys + yn).real.to(torch.complex64))
    gain = torch.where(torch.from_numpy(agc).to(dev)[:, None], gain, gain / torch.sqrt(p.clamp_min(1e-12))[:, None] * 0.3)
    out = 10 ** U(B, 1, a=-1.0, b=0.5) * gain
    keep = 10 ** (-U(B, 1, a=residual[0], b=residual[1]) / 20) if residual else torch.zeros(B, 1, device=dev)
    x = (out * (ys + yn).real).float()
    return x, (out * (ys + keep * yn).real).float()


class GpuStream(torch.utils.data.IterableDataset):
    """Whole batches from batch(), as data.GpuStream."""

    def __init__(self, seed: int, batch_size: int, snr=(-18.0, 20.0), p_real: float = 0.5, residual=(10.0, 15.0)):
        self.seed, self.batch_size, self.snr, self.p_real, self.residual = seed, batch_size, snr, p_real, residual

    def __iter__(self):
        w = torch.utils.data.get_worker_info()
        wid = w.id if w else 0
        rng = np.random.default_rng([self.seed, wid])
        gen = torch.Generator("cuda").manual_seed(self.seed * 1000 + wid)
        noise = NoiseBank() if self.p_real > 0 else None
        while True:
            yield tuple(v.cpu() for v in batch(self.batch_size, rng, gen, self.snr, noise, self.p_real,
                                               self.residual))


def fixed(n: int, seed: int, snr, residual=None):
    """A fixed set on the GPU: (inputs, clean targets, SNRs); half with
    real noise when NR_NOISE is there."""
    rng = np.random.default_rng(seed)
    gen = torch.Generator("cuda").manual_seed(seed)
    noise = NoiseBank() if os.path.exists(os.environ.get("NR_NOISE", "noise.i16")) else None
    xs, ys = [], []
    for i in range(0, n, 64):
        x, y = batch(min(64, n - i), rng, gen, snr, noise, 0.5, residual)
        xs.append(x.cpu())
        ys.append(y.cpu())
    return torch.cat(xs), torch.cat(ys), np.zeros(n)
