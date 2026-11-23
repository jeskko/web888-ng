# SPDX-License-Identifier: GPL-3.0-or-later
"""Training examples for the signal identifier: a clean signal of a random
class (modes.py) through a random channel, as the receiver would hand it
over: offset from the window's centre, Rayleigh fading (slow QSB or
auroral flutter) and multipath from ionochannel (Watterson, ITU-R
F.1487), LSB (spectrum inverted), neighbours of other
classes beside it, a birdie, impulse noise, white noise at an SNR in
2500 Hz, sometimes the edges of the receiver's passband, any level.
"""
from __future__ import annotations

import numpy as np
import torch

from modes import CLASSES, FS, GEN, HALF, TAU

try:
    import ionochannel as ic
except ImportError:
    # a checkout at IONOCHANNEL, by default beside this repository's
    import os
    import sys
    _here = os.path.dirname(os.path.abspath(__file__))
    sys.path.insert(0, os.path.join(os.environ.get("IONOCHANNEL", os.path.join(_here, "../../../../../ionochannel")),
                                    "python"))
    import ionochannel as ic

N = 24576                    # 4.096 s at FS


def active_power(x: np.ndarray) -> float:
    """Mean power over the louder half of 10 ms blocks (a signal with
    pauses is measured while on)."""
    b = np.abs(x[: len(x) // 60 * 60]) ** 2
    p = np.sort(b.reshape(-1, 60).mean(1))
    return float(p[len(p) // 2:].mean()) if p[-1] > 0 else 0.0


def channel(x: np.ndarray, rng) -> np.ndarray:
    """Fading and multipath on a clean signal (unit power): none, slow
    QSB (a Gaussian Doppler spectrum of 0.07-1.4 Hz spread) or auroral
    flutter (2-20 Hz); on 30 % a second path 0.5-4 ms later at -14..0 dB,
    fading on its own (steady without fading). Mean power stays 1."""
    kind = rng.choice(["none", "qsb", "flutter"], p=[0.45, 0.35, 0.2])
    spread = {"none": 0.0, "qsb": rng.uniform(0.07, 1.4), "flutter": rng.uniform(2, 20)}[kind]
    path = (lambda d, p: ic.scatter(d, p, spread)) if spread else ic.specular
    if rng.random() < 0.3:
        a2 = rng.uniform(0.2, 1.0) ** 2
        paths = [path(0.0, 1 / (1 + a2)), path(rng.uniform(0.5e-3, 4e-3), a2 / (1 + a2))]
    elif spread:
        # one path: its gain alone (the delay line's FFTs cost more than the rest)
        return x * ic.tap_gain(path(0.0, 1.0), len(x), FS, rng)
    else:
        return x
    return ic.Channel(FS, paths).apply(x, rng).astype(np.complex64)


def shift(x: np.ndarray, hz: float) -> np.ndarray:
    return x * np.exp(1j * TAU * hz * np.arange(len(x)) / FS).astype(np.complex64)


def example(rng, label: int | None = None, snr=(-6.0, 30.0)):
    c = int(rng.integers(len(CLASSES))) if label is None else label
    x = GEN[c](N, rng).astype(np.complex64)
    p = active_power(x)
    if p > 0:
        x = channel(x / np.sqrt(p), rng)
    # off centre: the scanner centres a wide signal on its loudest part
    # (speech: the low frequencies) as often as on its middle
    # (speech: anywhere in its band, as weak high frequencies don't show)
    w = max(300.0, (1.0 if CLASSES[c] in ("ssb", "am") else 0.6) * HALF[c])
    x = shift(x, rng.uniform(-w, w))
    # neighbours: other signals clear of this one's band, at least partly in the window
    if rng.random() < 0.5:
        for _ in range(rng.integers(1, 3)):
            k = int(rng.integers(1, len(CLASSES)))
            gap = HALF[c] + HALF[k] + rng.uniform(50, 400)
            off = rng.choice([-1, 1]) * rng.uniform(gap, max(gap + 1, 3000 + HALF[k] * 0.5))
            y = GEN[k](N, rng).astype(np.complex64)
            q = active_power(y)
            if q > 0:
                y = channel(y / np.sqrt(q), rng) * 10 ** (rng.uniform(-10, 10) / 20)
                x = x + shift(y, off)
    if rng.random() < 0.1:                    # a birdie
        x = x + shift(np.ones(N, np.complex64), rng.uniform(-2900, 2900)) * 10 ** rng.uniform(-1.5, 0.5)
    s = rng.uniform(*snr)
    sd = np.sqrt(10 ** (-s / 10) / 2500.0 * FS / 2)
    x = x + sd * (rng.normal(size=N) + 1j * rng.normal(size=N)).astype(np.complex64)
    if rng.random() < 0.1:                    # impulse noise
        m = rng.random(N) < rng.uniform(0.0005, 0.005)
        x[m] += sd * rng.uniform(5, 30, m.sum()) * np.exp(1j * rng.uniform(0, TAU, m.sum()))
    if HALF[c] < 2500 and rng.random() < 0.3:  # the receiver's passband edges (beyond this signal)
        X = np.fft.fft(x)
        f = np.fft.fftfreq(N, 1 / FS)
        lo = rng.uniform(-3000, -HALF[c] - 300)
        hi = rng.uniform(HALF[c] + 300, 3000)
        w = 1 / (1 + np.exp(-(f - lo) / 30)) / (1 + np.exp((f - hi) / 30))
        x = np.fft.ifft(X * np.maximum(w, 1e-3)).astype(np.complex64)
    if CLASSES[c] == "am" and rng.random() < 0.5:
        # AM through an SSB filter: its sidebands cut, each side on its own
        X = np.fft.fft(x)
        f = np.fft.fftfreq(N, 1 / FS)
        lo, hi = -rng.uniform(600, 2900), rng.uniform(600, 2900)
        w = 1 / (1 + np.exp(-(f - lo) / 30)) / (1 + np.exp((f - hi) / 30))
        x = np.fft.ifft(X * np.maximum(w, 1e-3)).astype(np.complex64)
    if rng.random() < 0.3:                    # LSB: the spectrum upside down
        x = np.conj(x)
    x = x * 10 ** rng.uniform(-2, 2)
    return x.astype(np.complex64), c, s


class Stream(torch.utils.data.IterableDataset):
    def __init__(self, seed: int):
        self.seed = seed

    def __iter__(self):
        w = torch.utils.data.get_worker_info()
        rng = np.random.default_rng([self.seed, w.id if w else 0])
        while True:
            x, c, _ = example(rng)
            yield torch.from_numpy(x), c


def fixed(n: int, seed: int, snr=(-6.0, 30.0)):
    """A fixed set: (x, labels, snr) with every class equally."""
    rng = np.random.default_rng(seed)
    xs, cs, ss = [], [], []
    for i in range(n):
        x, c, s = example(rng, label=i % len(CLASSES), snr=snr)
        xs.append(x)
        cs.append(c)
        ss.append(s)
    return torch.from_numpy(np.stack(xs)), torch.tensor(cs), np.array(ss)
