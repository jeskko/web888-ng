# SPDX-License-Identifier: GPL-3.0-or-later
"""Training examples for the SSB voice noise reduction: clean speech
(LibriSpeech, prep.py's bank) sent in SSB and received over a simulated
HF path, all from ionochannel:

* transmitter: a speech processor now and then (soft clipping), the
  transmit filter (lower edge 100-400 Hz, upper 2400-3000 Hz);
* channel: none, slow flat fading or an ITU-R F.520/F.1487 preset
  (selective fading, flutter);
* noise and interference: white or impulsive atmospheric noise (QRN,
  Vd 1-12 dB) at an SNR in 2500 Hz, and at times CW, RTTY or PSK31
  stations, a carrier with mains ripple, switching-supply or arcing
  noise, powerline bursts;
* receiver: mistuning (mostly a few Hz, sometimes up to ±200 Hz), the
  passband (150-400 to 2400-3000 Hz), the AGC on most.

The target is the same speech through the same transmitter, channel and
receiver (including the AGC's gain), without the noise: a mask can reach
it, and can't be taught to add sound. One example in ten is noise only.
"""
from __future__ import annotations

import os

import numpy as np
import torch

from common import FS

try:
    import ionochannel as ic
except ImportError:
    # a checkout at IONOCHANNEL, by default beside this repository's
    import sys
    _here = os.path.dirname(os.path.abspath(__file__))
    sys.path.insert(0, os.path.join(os.environ.get("IONOCHANNEL", os.path.join(_here, "../../../../../ionochannel")),
                                    "python"))
    import ionochannel as ic
from ionochannel.receiver import _agc  # noqa: E402  (the gain alone, to apply to both paths)

N = 48000                      # 4 s at FS
TAU = 2 * np.pi
PRESETS = ["good", "moderate", "poor", "flutter", "flat-fast", "mid-quiet", "mid-moderate", "mid-disturbed",
           "low-quiet", "low-moderate", "mid-nvis-disturbed", "high-quiet", "nvis-java-7mhz", "trough-5mhz",
           "polar-cap-5mhz"]


class Bank:
    """Speech at FS as int16 (prep.py): one memory-mapped file."""

    def __init__(self, path: str | None = None):
        path = path or os.environ.get("NR_SPEECH", "speech.i16")
        self.x = np.memmap(path, np.int16, "r")

    def clip(self, rng, n: int) -> np.ndarray:
        """n samples of speech, unit power over its louder half (pauses
        don't count); tried again if nearly silent."""
        for _ in range(8):
            i = int(rng.integers(0, len(self.x) - n))
            s = self.x[i:i + n].astype(np.float32) / 32768
            p = active_power(s)
            if p > 1e-6:
                return s / np.sqrt(p)
        return s / np.sqrt(max(active_power(s), 1e-9))


def active_power(x: np.ndarray, block: int = 120) -> float:
    """Mean power over the louder half of 10 ms blocks."""
    b = np.abs(x[: len(x) // block * block]) ** 2
    p = np.sort(b.reshape(-1, block).mean(1))
    return float(p[len(p) // 2:].mean())


def analytic(x: np.ndarray) -> np.ndarray:
    m = ic.channel.fast_len(2 * len(x))
    f = np.fft.fftfreq(m)
    return np.fft.ifft(np.fft.fft(x, m) * np.where(f > 0, 2.0, np.where(f == 0, 1.0, 0.0)))[:len(x)]


def transmit(s: np.ndarray, rng) -> np.ndarray:
    """Speech to the complex SSB baseband (audio f at +f Hz)."""
    if rng.random() < 0.4:                    # a speech processor: soft clipping
        g = 10 ** rng.uniform(0.3, 1.0)
        s = np.tanh(g * s / 3) * 3 / g * np.sqrt(g)
    lo, hi = rng.uniform(100, 400), rng.uniform(2400, 3000)
    z = ic.Receiver(FS, (lo, hi)).apply(analytic(s).astype(np.complex64))
    return z / np.sqrt(active_power(z))


def channel(z: np.ndarray, rng) -> np.ndarray:
    k = rng.random()
    if k < 0.2:
        return z
    if k < 0.45:
        paths = [ic.scatter(0.0, 1.0, float(np.exp(rng.uniform(np.log(0.05), np.log(1.0)))))]
    else:
        paths = ic.preset(PRESETS[int(rng.integers(len(PRESETS)))])
    return ic.Channel(FS, paths).apply(z, rng).astype(np.complex64)


def interference(n: int, rng) -> np.ndarray:
    """QRM and man-made noise, complex, at their own levels (dB against
    a unit signal); often none."""
    q = np.zeros(n, np.complex64)

    def lvl(a, b):
        return 10 ** (rng.uniform(a, b) / 20)

    off = lambda: rng.uniform(150, 2900)      # noqa: E731
    if rng.random() < 0.25:
        for _ in range(int(rng.integers(1, 3))):
            spec = ic.Cw(wpm=rng.uniform(12, 35), fade_hz=rng.choice([0.0, rng.uniform(0.05, 1.0)]))
            q += lvl(-15, 10) * ic.cw_noise((), n, FS, off(), rng, spec)
    if rng.random() < 0.08:
        q += lvl(-15, 5) * ic.rtty_noise((), n, FS, off(), rng)
    if rng.random() < 0.08:
        q += lvl(-15, 5) * ic.psk31_noise((), n, FS, off(), rng)
    if rng.random() < 0.2:                    # a carrier or a supply's line, mains ripple and drift
        spec = ic.Smps(mains_hz=float(rng.choice([50.0, 60.0])), am_depth=rng.uniform(0, 0.5),
                       fm_dev_hz=rng.uniform(0, 20), drift_hz=rng.uniform(0, 30))
        q += lvl(-15, 10) * ic.smps_noise((), n, FS, off(), rng, spec)
    if rng.random() < 0.05:
        q += lvl(-10, 10) * ic.arcing_noise((), n, FS, rng)
    if rng.random() < 0.05:
        q += lvl(-10, 5) * ic.bursts_noise((), n, FS, rng)
    return q


def example(rng, bank: Bank, snr=(-20.0, 15.0)):
    """(input audio, target audio, SNR in 2500 Hz), float32 at FS."""
    speech = rng.random() >= 0.1
    zs = channel(transmit(bank.clip(rng, N), rng), rng) if speech else np.zeros(N, np.complex64)
    s = rng.uniform(*snr)
    vd = 1.05 if rng.random() < 0.5 else rng.uniform(1.05, 12.0)
    # noise for this SNR against a unit signal (complex: spread over FS)
    zn = ic.qrn_noise((), N, FS, vd, rng) * np.sqrt(10 ** (-s / 10) * FS / 2500)
    zn = (zn + interference(N, rng)).astype(np.complex64)
    mis = rng.choice([rng.uniform(-20, 20), rng.uniform(-80, 80), rng.uniform(-200, 200)], p=[0.6, 0.3, 0.1])
    rx = ic.Receiver(FS, (rng.uniform(150, 400), rng.uniform(2400, 3000)), mis)
    ys, yn = rx.apply(zs), rx.apply(zn)
    if rng.random() < 0.7:
        g = _agc(ys + yn, FS, ic.Agc(decay_s=rng.uniform(0.1, 2.0), hang_s=rng.choice([0.0, 0.3])))
        ys, yn = ys * g, yn * g
    a = 10 ** rng.uniform(-1, 0.5)            # any level: the features don't care
    x = (a * (ys + yn).real).astype(np.float32)
    y = (a * ys.real).astype(np.float32)
    return x, y, s


# ------------------------------------------------------------ on the GPU

class NoiseBank:
    """Real HF noise at FS (noisebank.py): int16 streams and their index."""

    def __init__(self, path: str | None = None):
        path = path or os.environ.get("NR_NOISE", "noise.i16")
        self.x = np.memmap(path, np.int16, "r")
        self.idx = np.load(path + ".idx.npy")

    def clips(self, rng, k: int, n: int) -> np.ndarray:
        """k windows of n samples, each from a random stream, at unit
        power in 300-2700 Hz (as the synthetic noise's SNR counts it;
        what lies outside stays: KiwiSDRs pass up to about 3.5 kHz)."""
        out = np.empty((k, n), np.float32)
        f = np.fft.rfftfreq(n, 1 / FS)
        band = (f >= 300) & (f <= 2700)
        for i in range(k):
            a, ln = self.idx[rng.integers(len(self.idx))]
            j = a + rng.integers(0, ln - n)
            v = self.x[j:j + n].astype(np.float32)
            p = 2 * np.sum(np.abs(np.fft.rfft(v)[band]) ** 2) / n ** 2
            out[i] = v / (np.sqrt(p) + 1e-9)
        return out


def batch(B: int, rng, gen, bank: Bank, snr=(-20.0, 15.0), group: int = 8, noise: NoiseBank | None = None,
          p_real: float = 0.5):
    """(inputs, targets) (B, N) float32 on gen's device: example()'s
    recipe for a whole batch at once with ionochannel.torch. What the
    torch functions take as one value (a preset, a passband, Vd, an
    interferer's spec) is drawn per group of `group` rows; SNR,
    mistuning, levels and the on/off of each part per row."""
    import ionochannel.torch as ict
    from ionochannel.receiver import agc_block, agc_gains
    dev = gen.device
    U = lambda *s, a=0.0, b=1.0: a + (b - a) * torch.rand(*s, device=dev, generator=gen)  # noqa: E731
    s = np.stack([bank.clip(rng, N) for _ in range(B)])
    s[rng.random(B) < 0.1] = 0.0                                     # noise only
    x = torch.from_numpy(s).to(dev)
    # speech processor on 40 %
    g = 10 ** U(B, 1, a=0.3, b=1.0)
    x = torch.where(U(B, 1) < 0.4, torch.tanh(g * x / 3) * 3 / g * torch.sqrt(g), x)
    zs, zn = [], []
    real = torch.zeros(B, dtype=torch.bool, device=dev)     # rows with real noise (added after the receiver)
    snr_row = torch.zeros(B, 1, device=dev)
    for r0 in range(0, B, group):
        rows = min(group, B - r0)
        xg = x[r0:r0 + rows]
        z = ict.Receiver(FS, (rng.uniform(100, 400), rng.uniform(2400, 3000))).apply(
            ict._analytic(xg.to(torch.complex64), ic.channel.fast_len(2 * N))[:, :N])
        p = active_power_t(z)
        z = z / torch.sqrt(torch.where(p > 0, p, 1.0))[:, None]
        k = rng.random()
        if k >= 0.2:
            paths = ([ic.scatter(0.0, 1.0, float(np.exp(rng.uniform(np.log(0.05), np.log(1.0)))))] if k < 0.45
                     else ic.preset(PRESETS[int(rng.integers(len(PRESETS)))]))
            z = ict.Channel(FS, paths).apply(z, gen)
        zs.append(z)
        # noise at a per-row SNR, Vd per group
        vd = 1.05 if rng.random() < 0.5 else rng.uniform(1.05, 12.0)
        sn = U(rows, 1, a=snr[0], b=snr[1])
        snr_row[r0:r0 + rows] = sn
        if noise is not None and rng.random() < p_real:
            real[r0:r0 + rows] = True
            zn.append(interference_t(rows, rng, gen, U))
            continue
        n = ict.qrn_noise((rows,), N, FS, vd, gen) * torch.sqrt(10 ** (-sn / 10) * FS / 2500)
        zn.append(n + interference_t(rows, rng, gen, U))
    zs, zn = torch.cat(zs), torch.cat(zn)
    # receiver: passband per group, mistuning per row
    mis = torch.where(U(B) < 0.6, U(B, a=-20, b=20), torch.where(U(B) < 0.75, U(B, a=-80, b=80),
                                                                  U(B, a=-200, b=200)))
    ys, yn = [], []
    for r0 in range(0, B, group):
        rx = ict.Receiver(FS, (rng.uniform(150, 400), rng.uniform(2400, 3000)), mis[r0:r0 + group].double())
        ys.append(rx.apply(zs[r0:r0 + group]))
        yn.append(rx.apply(zn[r0:r0 + group]))
    ys, yn = torch.cat(ys), torch.cat(yn)
    if bool(real.any()):
        # real noise, already through a receiver: on the audio, at the row's
        # SNR against the speech's active power (a unit signal's, if none)
        ri = torch.nonzero(real)[:, 0]
        nz = torch.from_numpy(noise.clips(rng, len(ri), N)).to(dev)
        ps = active_power_t(ys[ri].real.to(torch.complex64))
        ps = torch.where(ps > 0, ps, torch.full_like(ps, 0.5))
        yn[ri] = yn[ri] + (nz * torch.sqrt(ps[:, None] / 10 ** (snr_row[ri] / 10))).to(yn.dtype)
    # the AGC on 70 %: its gain from the sum, on both paths (one row at a
    # time: the plain-float loop beats numpy's over rows)
    a = ic.Agc(decay_s=rng.uniform(0.1, 2.0), hang_s=float(rng.choice([0.0, 0.3])))
    b = agc_block(FS)
    nb = -(-N // b)
    env = torch.zeros((B, nb * b), device=dev)
    env[:, :N] = (ys + yn).abs()
    peaks = env.reshape(B, nb, b).amax(-1).double().cpu().numpy()
    gk = np.ones((B, nb))
    for r in np.flatnonzero(rng.random(B) < 0.7):
        gk[r] = agc_gains(peaks[r], FS, a)
    gk = torch.as_tensor(gk, device=dev, dtype=torch.float32)
    prev = torch.cat([gk[:, :1], gk[:, :-1]], dim=-1)
    ramp = torch.arange(1, b + 1, device=dev) / b
    gain = (prev[..., None] + (gk - prev)[..., None] * ramp).reshape(B, nb * b)[:, :N]
    lvl = 10 ** U(B, 1, a=-1.0, b=0.5)
    return (lvl * gain * (ys + yn).real).float(), (lvl * gain * ys.real).float()


def active_power_t(z: torch.Tensor, block: int = 120) -> torch.Tensor:
    b = (z[:, : z.shape[1] // block * block].abs() ** 2).reshape(z.shape[0], -1, block).mean(-1)
    b, _ = torch.sort(b, dim=-1)
    return b[:, b.shape[1] // 2:].mean(-1)


def interference_t(rows: int, rng, gen, U) -> torch.Tensor:
    """interference() for a group: each kind made only for the rows that
    have it."""
    import ionochannel.torch as ict
    dev = gen.device
    q = torch.zeros((rows, N), dtype=torch.complex64, device=dev)

    def add(p, a, b, make):
        idx = np.flatnonzero(rng.random(rows) < p)
        if len(idx):
            k = len(idx)
            lv = (10 ** (U(k, 1, a=a, b=b) / 20)).to(torch.complex64)
            q[torch.as_tensor(idx, device=dev)] += lv * make(k)

    off = lambda k: U(k, a=150.0, b=2900.0).double()  # noqa: E731
    for _ in range(2):
        spec = ic.Cw(wpm=rng.uniform(12, 35), fade_hz=float(rng.choice([0.0, rng.uniform(0.05, 1.0)])))
        add(0.18, -15, 10, lambda k: ict.cw_noise((k,), N, FS, off(k), gen, spec))
    add(0.08, -15, 5, lambda k: ict.rtty_noise((k,), N, FS, off(k), gen))
    add(0.08, -15, 5, lambda k: ict.psk31_noise((k,), N, FS, off(k), gen))
    spec = ic.Smps(mains_hz=float(rng.choice([50.0, 60.0])), am_depth=rng.uniform(0, 0.5),
                   fm_dev_hz=rng.uniform(0, 20), drift_hz=rng.uniform(0, 30))
    add(0.2, -15, 10, lambda k: ict.smps_noise((k,), N, FS, rng.uniform(150, 2900), gen, spec))
    add(0.05, -10, 10, lambda k: ict.arcing_noise((k,), N, FS, gen))
    add(0.05, -10, 5, lambda k: ict.bursts_noise((k,), N, FS, gen))
    return q


class GpuStream(torch.utils.data.IterableDataset):
    """Whole batches from batch(), each worker on the GPU with its own
    generator (use with batch_size=None and the 'spawn' context);
    returned on the CPU."""

    def __init__(self, seed: int, batch_size: int, snr=(-20.0, 15.0), p_real: float = 0.0):
        self.seed, self.batch_size, self.snr, self.p_real = seed, batch_size, snr, p_real

    def __iter__(self):
        w = torch.utils.data.get_worker_info()
        wid = w.id if w else 0
        rng = np.random.default_rng([self.seed, wid])
        gen = torch.Generator("cuda").manual_seed(self.seed * 1000 + wid)
        bank = Bank()
        noise = NoiseBank() if self.p_real > 0 else None
        while True:
            x, y = batch(self.batch_size, rng, gen, bank, self.snr, noise=noise, p_real=self.p_real)
            yield x.cpu(), y.cpu()


class Stream(torch.utils.data.IterableDataset):
    def __init__(self, seed: int):
        self.seed = seed

    def __iter__(self):
        w = torch.utils.data.get_worker_info()
        rng = np.random.default_rng([self.seed, w.id if w else 0])
        bank = Bank()
        while True:
            x, y, _ = example(rng, bank)
            yield torch.from_numpy(x), torch.from_numpy(y)


def fixed(n: int, seed: int, snr):
    """A fixed set: (inputs, targets, SNRs)."""
    rng = np.random.default_rng(seed)
    bank = Bank(os.environ.get("NR_SPEECH_VAL"))
    xs, ys, ss = zip(*(example(rng, bank, snr) for _ in range(n)))
    return torch.from_numpy(np.stack(xs)), torch.from_numpy(np.stack(ys)), np.array(ss)
