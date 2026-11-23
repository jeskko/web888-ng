# SPDX-License-Identifier: GPL-3.0-or-later
"""STOI, the short-time objective intelligibility measure (Taal, Hendriks,
Heusdens, Jensen, IEEE TASLP 19(7), 2011): the correlation of clean and
processed speech envelopes in 15 one-third-octave bands over 384 ms
segments, after silent frames are dropped. 0..1, higher is more
intelligible; a predictor of listeners' word scores, unlike an ASR's.

Written from the paper's description (10 kS/s, 256-sample Hann frames,
hop 128, 512-point FFT, bands from 150 Hz, 30-frame segments, the
processed envelope clipped at -15 dB SDR, frames 40 dB below the clean
signal's loudest dropped).
"""
import numpy as np
from scipy.signal import resample_poly

FS = 10000
N_FRAME = 256
NFFT = 512
BANDS = 15
F_MIN = 150.0
SEG = 30
BETA = -15.0
DYN = 40.0


def _octave_matrix():
    f = np.linspace(0, FS, NFFT + 1)[: NFFT // 2 + 1]
    k = np.arange(BANDS)
    cf = F_MIN * 2 ** (k / 3)
    lo, hi = cf * 2 ** (-1 / 6), cf * 2 ** (1 / 6)
    m = np.zeros((BANDS, len(f)))
    for i in range(BANDS):
        a = np.argmin(np.abs(f - lo[i]))
        b = np.argmin(np.abs(f - hi[i]))
        m[i, a:b] = 1
    return m


OBM = _octave_matrix()
WIN = np.hanning(N_FRAME + 2)[1:-1]


def _frames(x):
    n = (len(x) - N_FRAME) // (N_FRAME // 2) + 1
    idx = np.arange(N_FRAME)[None, :] + (N_FRAME // 2) * np.arange(n)[:, None]
    return x[idx] * WIN


def _drop_silent(x, y):
    fx, fy = _frames(x), _frames(y)
    e = 20 * np.log10(np.linalg.norm(fx, axis=1) + 1e-12)
    keep = e > e.max() - DYN
    fx, fy = fx[keep], fy[keep]
    n = (len(fx) - 1) * (N_FRAME // 2) + N_FRAME

    def ola(fr):
        out = np.zeros(n)
        for i, v in enumerate(fr):
            out[i * N_FRAME // 2: i * N_FRAME // 2 + N_FRAME] += v
        return out

    return ola(fx), ola(fy)


def stoi(clean: np.ndarray, proc: np.ndarray, fs: int) -> float:
    """STOI of `proc` against `clean` (same length, same rate fs)."""
    n = min(len(clean), len(proc))
    x, y = clean[:n].astype(np.float64), proc[:n].astype(np.float64)
    if fs != FS:
        g = np.gcd(fs, FS)
        x, y = resample_poly(x, FS // g, fs // g), resample_poly(y, FS // g, fs // g)
    x, y = _drop_silent(x, y)
    X = np.abs(np.fft.rfft(_frames(x), NFFT, axis=1)) ** 2
    Y = np.abs(np.fft.rfft(_frames(y), NFFT, axis=1)) ** 2
    Xb, Yb = np.sqrt(X @ OBM.T).T, np.sqrt(Y @ OBM.T).T          # (bands, frames)
    if Xb.shape[1] < SEG:
        return float("nan")
    c = 10 ** (-BETA / 20)
    d = []
    for m in range(SEG, Xb.shape[1] + 1):
        xs, ys = Xb[:, m - SEG:m], Yb[:, m - SEG:m]
        a = np.linalg.norm(xs, axis=1, keepdims=True) / (np.linalg.norm(ys, axis=1, keepdims=True) + 1e-12)
        yn = np.minimum(ys * a, xs * (1 + c))
        xm = xs - xs.mean(1, keepdims=True)
        ym = yn - yn.mean(1, keepdims=True)
        r = (xm * ym).sum(1) / (np.linalg.norm(xm, axis=1) * np.linalg.norm(ym, axis=1) + 1e-12)
        d.append(r.mean())
    return float(np.mean(d))


def estoi(clean: np.ndarray, proc: np.ndarray, fs: int) -> float:
    """Extended STOI (Jensen and Taal, IEEE/ACM TASLP 24(11), 2016): the
    segments' envelopes normalised over time per band, then over bands
    per frame, correlated as whole spectro-temporal patterns; no
    clipping. Better than STOI for nonlinear processing such as masks."""
    n = min(len(clean), len(proc))
    x, y = clean[:n].astype(np.float64), proc[:n].astype(np.float64)
    if fs != FS:
        g = np.gcd(fs, FS)
        x, y = resample_poly(x, FS // g, fs // g), resample_poly(y, FS // g, fs // g)
    x, y = _drop_silent(x, y)
    X = np.abs(np.fft.rfft(_frames(x), NFFT, axis=1)) ** 2
    Y = np.abs(np.fft.rfft(_frames(y), NFFT, axis=1)) ** 2
    Xb, Yb = np.sqrt(X @ OBM.T).T, np.sqrt(Y @ OBM.T).T
    if Xb.shape[1] < SEG:
        return float("nan")

    def norm(a, axis):
        a = a - a.mean(axis, keepdims=True)
        return a / (np.linalg.norm(a, axis=axis, keepdims=True) + 1e-12)

    d = []
    for m in range(SEG, Xb.shape[1] + 1):
        xs = norm(norm(Xb[:, m - SEG:m], 1), 0)
        ys = norm(norm(Yb[:, m - SEG:m], 1), 0)
        d.append((xs * ys).sum() / SEG)
    return float(np.mean(d))
