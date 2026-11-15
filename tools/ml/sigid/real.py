# SPDX-License-Identifier: GPL-3.0-or-later
"""Real signals for checking the signal identifier: windows of a wideband
IQ recording (HackRF cs8) or a channelizer-bin capture (f32 I/Q) at a
station's frequency, as 4.096 s of 6 kS/s baseband.

    python3 real.py survey REC.cs8 RATE CENTRE_HZ T0 HZ [HZ ...]   # waterfalls (PNG)
    python3 real.py eval MODEL.pt LABELS.txt                         # confusion on labelled windows

LABELS.txt: lines `file rate centre_hz class hz t0 [t1 step]` (times in s;
several windows from t0 to t1 every step). `file` may be a cs8 recording
(int8 I/Q) or an .iq bin capture (f32 I/Q).
"""
from __future__ import annotations

import sys

import numpy as np
from scipy.signal import resample_poly

FS = 6000
N = 24576


def window(path: str, rate: float, centre: float, hz: float, t0: float, n: int = N) -> np.ndarray:
    """n samples at FS of the channel at hz from t0 s."""
    f32 = path.endswith(".iq")
    bps = 8 if f32 else 2
    need = int((n / FS + 0.1) * rate)
    a = int(t0 * rate)
    raw = np.fromfile(path, dtype=np.float32 if f32 else np.int8, count=2 * need, offset=a * bps)
    x = raw[0::2].astype(np.float32) + 1j * raw[1::2].astype(np.float32)
    x = x * np.exp(-2j * np.pi * (hz - centre) / rate * np.arange(len(x)))
    from fractions import Fraction
    r = Fraction(FS, int(rate)).limit_denominator(10000)
    y = resample_poly(x, r.numerator, r.denominator)
    k = int(0.05 * FS)
    return y[k:k + n].astype(np.complex64)


def survey(path, rate, centre, t0, hzs):
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    fig, ax = plt.subplots(1, len(hzs), figsize=(3 * len(hzs), 4))
    ax = np.atleast_1d(ax)
    for a, hz in zip(ax, hzs):
        x = window(path, rate, centre, hz, t0)
        f, t, S = __import__("scipy.signal", fromlist=["spectrogram"]).spectrogram(
            x, FS, nperseg=256, noverlap=192, return_onesided=False)
        S = np.fft.fftshift(S, axes=0)
        a.imshow(10 * np.log10(S + 1e-12), aspect="auto", origin="lower", extent=[0, t[-1], -3000, 3000],
                 vmin=np.percentile(10 * np.log10(S + 1e-12), 20), vmax=np.percentile(10 * np.log10(S + 1e-12), 99.5))
        a.set_title(f"{hz / 1e3:.2f} kHz")
    plt.tight_layout()
    out = "survey.png"
    plt.savefig(out, dpi=60)
    print(out)


def load_labels(path):
    items = []
    for line in open(path):
        line = line.split("#")[0].split()
        if not line:
            continue
        f, rate, centre, cls, hz, t0 = line[:6]
        t1, step = (float(line[6]), float(line[7])) if len(line) > 7 else (float(t0), 1.0)
        t = float(t0)
        while t <= t1 + 1e-9:
            items.append((f, float(rate), float(centre), cls, float(hz), t))
            t += step
    return items


def evaluate(model_path, labels_path):
    import torch
    from common import SigId, features
    from modes import CLASSES
    dev = "cuda" if torch.cuda.is_available() else "cpu"
    model = SigId().to(dev)
    model.load_state_dict(torch.load(model_path, map_location=dev))
    model.eval()
    items = load_labels(labels_path)
    cache = labels_path + ".npy"                      # the windows, extracted once
    import os
    if os.path.exists(cache) and os.path.getmtime(cache) > os.path.getmtime(labels_path):
        xs = np.load(cache)
    else:
        xs = np.stack([window(f, r, c, hz, t) for f, r, c, _, hz, t in items])
        np.save(cache, xs)
    with torch.no_grad():
        p = np.concatenate([torch.softmax(model(*features(torch.from_numpy(xs[i:i + 64]).to(dev))), 1).cpu().numpy()
                            for i in range(0, len(xs), 64)])
    by = {}
    for (f, r, c, cls, hz, t), q in zip(items, p):
        k = int(q.argmax())
        by.setdefault((cls, hz), []).append((CLASSES[k], q[k]))
    ok = n = 0
    for (cls, hz), v in sorted(by.items()):
        good = sum(1 for g, _ in v if g == cls)
        ok += good
        n += len(v)
        top = {}
        for g, _ in v:
            top[g] = top.get(g, 0) + 1
        print(f"{cls:<11} {hz / 1e3:10.2f} kHz  {good}/{len(v)}  " +
              " ".join(f"{g}:{c}" for g, c in sorted(top.items(), key=lambda z: -z[1])))
    print(f"all: {ok}/{n} ({100 * ok / n:.0f} %)")


if __name__ == "__main__":
    if sys.argv[1] == "survey":
        survey(sys.argv[2], float(sys.argv[3]), float(sys.argv[4]), float(sys.argv[5]), [float(h) for h in sys.argv[6:]])
    else:
        evaluate(sys.argv[2], sys.argv[3])
