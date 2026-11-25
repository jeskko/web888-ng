# SPDX-License-Identifier: GPL-3.0-or-later
"""Training examples for the AM broadcast noise reduction (speech and
music), at the page's 12 kS/s, made on the GPU like data.py's. The chain
runs at FS2 = 24 kS/s complex (room for stations 9-10 kHz away):

* programme: speech (LibriSpeech, prep.py's bank) or music (MUSAN,
  musicbank.py), now and then speech over music or one turning into
  the other (a presenter between songs); broadcast processing (a tilt,
  a compressor, a clipper) and the transmitter's audio lowpass at
  2.5-5 kHz;
* AM: a carrier, 30-100 % modulation at the peaks, a few Hz off;
* channel: none, flat fading or an ionochannel preset (selective fading:
  the detector's distortion is in the target too, the model mustn't
  undo it);
* noise and interference: white or impulsive (QRN) or real HF noise
  (Paderborn pauses) at an SNR (the programme's power against the noise
  in the receiver's passband); at times stations 9 or 10 kHz away (their
  splatter, "monkey chatter"), a whistle that drifts and wobbles, CW,
  switching-supply hash, arcing, powerline bursts;
* receiver: a filter of 2.5-5 kHz either side of the carrier, or one
  sideband (SAL/SAR), then the detector: envelope (AM) or synchronous
  (the carrier's phase and level from the signal itself, a 20 Hz
  lowpass), the audio relative to the carrier (the carrier AGC), DC
  taken out.

The target is the same chain without the noise and interference (the
same carrier estimate: in the synchronous detector the two parts add),
and, `residual` dB below, the noise. One example in ten has no
programme (the carrier only).
"""
from __future__ import annotations

import os

import numpy as np
import torch

from common import FS
from data import N, PRESETS, Bank, NoiseBank, active_power_t, ic

FS2 = 2 * FS
N2 = 2 * N
TAU = 2 * np.pi


class MusicBank(Bank):
    """Music at FS as int16 (musicbank.py)."""

    def __init__(self, path: str | None = None):
        path = path or os.environ.get("NR_MUSIC", "music.i16")
        self.x = np.memmap(path, np.int16, "r")


def _lowpass(x: torch.Tensor, hz, fs: float, order: int = 8) -> torch.Tensor:
    """x (B, n) real or complex through a zero-phase Butterworth-like
    magnitude response with its corner at `hz` (scalar or (B, 1))."""
    n = x.shape[-1]
    m = 1 << int(np.ceil(np.log2(2 * n)))
    f = torch.fft.fftfreq(m, 1 / fs, device=x.device).abs()[None, :]
    h = 1 / torch.sqrt(1 + (f / hz) ** (2 * order))
    return torch.fft.ifft(torch.fft.fft(x, m) * h, m)[..., :n] if x.is_complex() else \
        torch.fft.ifft(torch.fft.fft(x.to(torch.complex64), m) * h, m)[..., :n].real


def _up2(x: torch.Tensor) -> torch.Tensor:
    """(B, n) real at FS to (B, 2n) at FS2 (band-limited)."""
    n = x.shape[-1]
    X = torch.fft.rfft(x)
    Y = torch.zeros((x.shape[0], n + 1), dtype=X.dtype, device=x.device)
    Y[:, : X.shape[1]] = X
    return torch.fft.irfft(Y, 2 * n) * 2


def broadcast(x: torch.Tensor, U) -> torch.Tensor:
    """A broadcaster's processing per row: a tilt (-3..+3 dB/octave
    around 1 kHz), a compressor (2-10:1, 10-30 dB under the peaks), a
    clipper (0-12 dB into it), the audio lowpass (2.5-5 kHz); peak 1."""
    B, n = x.shape
    dev = x.device
    f = torch.fft.rfftfreq(n, 1 / FS, device=dev)[None, :]
    tilt = U(B, 1, a=-3.0, b=3.0) * torch.log2(f.clamp_min(50.0) / 1000.0)
    x = torch.fft.irfft(torch.fft.rfft(x) * 10 ** (tilt / 20), n)
    blk = 60
    m = n // blk
    e = x[:, : m * blk].abs().reshape(B, m, blk).amax(-1)
    rel = U(B, 1, a=0.05, b=0.5) * FS / blk
    k = torch.arange(64, device=dev)[None, :]
    kern = torch.exp(-k / rel)
    ef = torch.fft.irfft(torch.fft.rfft(e, 2 * m) * torch.fft.rfft(kern / kern.sum(1, keepdim=True), 2 * m), 2 * m)[:, :m]
    env = torch.maximum(e, ef * 1.5).clamp_min(1e-6)
    top = torch.quantile(env, 0.95, dim=1, keepdim=True)
    thr = top * 10 ** (-U(B, 1, a=10.0, b=30.0) / 20)
    ratio = U(B, 1, a=2.0, b=10.0)
    g = torch.where(env > thr, (env / thr) ** (1 / ratio - 1), torch.ones_like(env))
    g = torch.repeat_interleave(g, blk, dim=1)
    g = torch.cat([g, g[:, -1:].expand(B, n - m * blk)], dim=1)
    x = x * g
    drive = 10 ** (U(B, 1, a=0.0, b=12.0) / 20)
    pk = torch.quantile(x.abs(), 0.99, dim=1, keepdim=True).clamp_min(1e-6)
    x = torch.tanh(drive * x / pk) * pk
    x = _lowpass(x, U(B, 1, a=2500.0, b=5000.0), FS)
    return x / x.abs().amax(1, keepdim=True).clamp_min(1e-9)


def programme(B: int, rng, speech: Bank, music: Bank | None, p_music: float = 0.4) -> np.ndarray:
    """(B, N) programme audio at FS: music, speech, (10 %) speech over
    music, or (10 %) one turning into the other partway through (a
    presenter between songs: a hard cut or a crossfade of up to 1 s, the
    levels up to 6 dB apart)."""
    out = np.empty((B, N), np.float32)
    for i in range(B):
        k = rng.random()
        if music is not None and k < p_music:
            out[i] = music.clip(rng, N)
        elif music is not None and k < p_music + 0.1:
            out[i] = speech.clip(rng, N) + 10 ** (-rng.uniform(6, 20) / 20) * music.clip(rng, N)
        elif music is not None and k < p_music + 0.2:
            a, b = speech.clip(rng, N), music.clip(rng, N) * 10 ** (rng.uniform(-6, 6) / 20)
            if rng.random() < 0.5:
                a, b = b, a
            at, fade = int(rng.integers(N // 5, 4 * N // 5)), int(rng.uniform(0, 1) * FS)
            w = np.clip((np.arange(N) - at) / max(fade, 1) + 0.5, 0, 1).astype(np.float32)
            out[i] = (1 - w) * a + w * b
        else:
            out[i] = speech.clip(rng, N)
    return out


def batch(B: int, rng, gen, speech: Bank, music: Bank | None, snr=(-5.0, 25.0), noise: NoiseBank | None = None,
          p_real: float = 0.5, residual=(10.0, 15.0), group: int = 8):
    """(inputs, targets) (B, N) float32 on gen's device."""
    import ionochannel.torch as ict
    dev = gen.device
    U = lambda *s, a=0.0, b=1.0: a + (b - a) * torch.rand(*s, device=dev, generator=gen)  # noqa: E731
    a = torch.from_numpy(programme(B, rng, speech, music)).to(dev)
    a = broadcast(a, U)
    a = torch.where(torch.from_numpy(rng.random(B) < 0.1).to(dev)[:, None], torch.zeros_like(a), a)
    a2 = _up2(a)
    t = torch.arange(N2, device=dev) / FS2
    # AM: peak modulation 30-100 %, the carrier a few Hz off
    depth = U(B, 1, a=0.3, b=1.0)
    off = U(B, 1, a=-10.0, b=10.0)
    zs = ((1 + depth * a2) * torch.exp(1j * (TAU * off * t + U(B, 1, b=TAU)))).to(torch.complex64)
    # channel per group
    for r0 in range(0, B, group):
        k = rng.random()
        if k >= 0.25:
            paths = ([ic.scatter(0.0, 1.0, float(np.exp(rng.uniform(np.log(0.05), np.log(1.0)))))] if k < 0.5
                     else ic.preset(PRESETS[int(rng.integers(len(PRESETS)))]))
            zs[r0:r0 + group] = ict.Channel(FS2, paths).apply(zs[r0:r0 + group], gen)
    # the receiver's passband: both sidebands (2.5-5 kHz) or one (SAL/SAR)
    hi = rng.uniform(2500, 5000, B // group + 1)
    side = rng.choice([0, 0, 0, -1, 1], B // group + 1)
    sync = (rng.random(B // group + 1) < 0.6) | (side != 0)
    # noise: the SNR of the programme (its power, carrier excluded) against
    # the noise in the passband
    sn = U(B, 1, a=snr[0], b=snr[1])
    pa = (depth ** 2 * active_power_t(a.to(torch.complex64))[:, None]).clamp_min(1e-4)
    width = torch.from_numpy(np.repeat(np.where(side == 0, 2 * hi, hi), group)[:B]).to(dev).float()[:, None]
    n0 = pa * 10 ** (-sn / 10) / width                        # noise power per Hz
    zn = torch.zeros_like(zs)
    for r0 in range(0, B, group):
        rows = min(group, B - r0)
        if noise is not None and rng.random() < p_real:
            # real noise (unit power in 300-2700 Hz, one-sided audio): two
            # clips as I and Q (a density of 1/2400 per Hz), at FS2, and
            # white 10 dB under them
            c = torch.from_numpy(noise.clips(rng, 2 * rows, N)).to(dev)
            q = torch.complex(_up2(c[:rows]), _up2(c[rows:])) * 2400 ** 0.5
            zn[r0:r0 + rows] = q + 0.3 * ict.qrn_noise((rows,), N2, FS2, 1.05, gen) * FS2 ** 0.5
        else:
            vd = 1.05 if rng.random() < 0.5 else rng.uniform(1.05, 12.0)
            zn[r0:r0 + rows] = ict.qrn_noise((rows,), N2, FS2, vd, gen) * FS2 ** 0.5
    zn = zn * torch.sqrt(n0)                                  # from a density of 1 per Hz

    def add(p, lo, hi_, make):
        idx = np.flatnonzero(rng.random(B) < p)
        if len(idx):
            ix = torch.as_tensor(idx, device=dev)
            lv = torch.sqrt(10 ** (U(len(idx), 1, a=lo, b=hi_) / 10)).to(torch.complex64)
            zn[ix] += lv * make(len(idx), ix)

    def neighbour(k, ix):
        o = torch.as_tensor(rng.choice([-10000.0, -9000.0, 9000.0, 10000.0], k), device=dev).double()
        aud = _up2(broadcast(torch.from_numpy(programme(k, rng, speech, music)).to(dev), U))
        spec = ic.Am(depth=rng.uniform(0.5, 1.0), audio_hz=rng.uniform(4000, 6000))
        return ict.am_noise((k,), N2, FS2, o, gen, spec, audio=aud)

    def whistle(k, ix):
        f0 = U(k, 1, a=-4500.0, b=4500.0)
        wob = U(k, 1, a=0.0, b=30.0) * torch.sin(TAU * U(k, 1, a=0.1, b=5.0) * t + U(k, 1, b=TAU))
        drift = U(k, 1, a=-100.0, b=100.0) * t / t[-1]
        ph = TAU * torch.cumsum((f0 + wob + drift) / FS2, dim=1)
        return torch.exp(1j * ph).to(torch.complex64)

    add(0.35, -25, 0, neighbour)                                  # dBc: against our carrier (power 1)
    add(0.25, -40, -10, whistle)
    for _ in range(2):
        spec = ic.Cw(wpm=rng.uniform(12, 35), fade_hz=float(rng.choice([0.0, rng.uniform(0.05, 1.0)])))
        add(0.08, -35, -10, lambda k, ix: ict.cw_noise((k,), N2, FS2, U(k, a=-4500.0, b=4500.0).double(), gen, spec))
    spec = ic.Smps(mains_hz=float(rng.choice([50.0, 60.0])), am_depth=rng.uniform(0, 0.5),
                   fm_dev_hz=rng.uniform(0, 20), drift_hz=rng.uniform(0, 30))
    add(0.15, -40, -15, lambda k, ix: ict.smps_noise((k,), N2, FS2, rng.uniform(-4500, 4500), gen, spec))
    add(0.05, -30, -5, lambda k, ix: ict.arcing_noise((k,), N2, FS2, gen))
    add(0.05, -30, -10, lambda k, ix: ict.bursts_noise((k,), N2, FS2, gen))
    # receiver: the filter, to FS, the detector
    ys, yx = torch.empty((B, N), device=dev), torch.empty((B, N), device=dev)
    for gi, r0 in enumerate(range(0, B, group)):
        sl = slice(r0, min(B, r0 + group))
        band = (-hi[gi], hi[gi]) if side[gi] == 0 else ((50.0, hi[gi]) if side[gi] > 0 else (-hi[gi], -50.0))
        rx = ict.Receiver(FS2, band, transition_hz=300.0)
        s = rx.apply(zs[sl])[:, ::2]
        x = s + rx.apply(zn[sl])[:, ::2]
        # the carrier as the detector sees it: phase and level from the
        # whole signal, 20 Hz lowpass
        c = _lowpass(x, 20.0, FS, order=2)
        # (the carrier AGC's gain at most 20 dB over the row's median)
        lvl = torch.maximum(c.abs(), 0.1 * c.abs().median(dim=1, keepdim=True).values).clamp_min(1e-6)
        if sync[gi]:
            u = c.conj() / lvl
            k = 1.0 if side[gi] == 0 else 2.0
            ds, dx = k * (s * u).real / lvl, k * (x * u).real / lvl
        else:
            ds, dx = s.abs() / lvl, x.abs() / lvl
        # DC out (the carrier), as the demodulators do
        ys[sl] = ds - _lowpass(ds, 10.0, FS, order=1)
        yx[sl] = dx - _lowpass(dx, 10.0, FS, order=1)
    keep = 10 ** (-U(B, 1, a=residual[0], b=residual[1]) / 20) if residual else torch.zeros(B, 1, device=dev)
    g = 10 ** U(B, 1, a=-1.3, b=-0.3)
    return (g * yx).float(), (g * (ys + keep * (yx - ys))).float()


class GpuStream(torch.utils.data.IterableDataset):
    """Whole batches from batch(), as data.GpuStream."""

    def __init__(self, seed: int, batch_size: int, snr=(-5.0, 25.0), p_real: float = 0.5, residual=(10.0, 15.0)):
        self.seed, self.batch_size, self.snr, self.p_real, self.residual = seed, batch_size, snr, p_real, residual

    def __iter__(self):
        w = torch.utils.data.get_worker_info()
        wid = w.id if w else 0
        rng = np.random.default_rng([self.seed, wid])
        gen = torch.Generator("cuda").manual_seed(self.seed * 1000 + wid)
        speech, music = Bank(), MusicBank()
        noise = NoiseBank() if self.p_real > 0 else None
        while True:
            yield tuple(v.cpu() for v in batch(self.batch_size, rng, gen, speech, music, self.snr, noise,
                                               self.p_real, self.residual))


def fixed(n: int, seed: int, snr, residual=None):
    """A fixed set on the GPU: (inputs, clean targets, SNRs), from the
    validation speech (NR_SPEECH_VAL) and music (NR_MUSIC_VAL)."""
    rng = np.random.default_rng(seed)
    gen = torch.Generator("cuda").manual_seed(seed)
    speech = Bank(os.environ.get("NR_SPEECH_VAL"))
    music = MusicBank(os.environ.get("NR_MUSIC_VAL"))
    noise = NoiseBank() if os.path.exists(os.environ.get("NR_NOISE", "noise.i16")) else None
    xs, ys = [], []
    for i in range(0, n, 32):
        x, y = batch(min(32, n - i), rng, gen, speech, music, snr, noise, 0.5, residual)
        xs.append(x.cpu())
        ys.append(y.cpu())
    return torch.cat(xs), torch.cat(ys), np.zeros(n)
