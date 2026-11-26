# SPDX-License-Identifier: GPL-3.0-or-later
"""Real training pairs: the Paderborn database's train receptions and,
for each, its clean source as the speech inside it: delayed by the
reception's lag and passed through the reception's own channel response,
estimated per frequency as (speech-frame power - pause power) / clean
power (Welch, 256 points, smoothed over 5 bins). Mistuning and fading are
not modelled. Receptions whose speech isn't at least 10 dB below their
noise in 300-2700 Hz (in-band SNR >= -10 dB) are kept.

Both at 12 kS/s, one int16 file each (noisy and speech, the same scale
per reception) and an index (.idx.npy: start, length).

    python3 pairbank.py /data/nr/paderborn/ham_radio pairs [--align] [--fill]
    # -> pairs.x.i16, pairs.s.i16, pairs.idx.npy

With --align, each utterance (the annotation's activity spans) at its own
lag, found from band log-power envelopes (phase-free) within 300 ms of
the reception's global lag; otherwise one global lag from waveform
correlation, which a few Hz of mistuning (the SSB phase turning a few
times a second) misleads: in 60 train receptions half the utterances
were 10 ms or more off, some 40-60 ms, and the lag drifts within a
file (online receivers' buffering and clocks).

With --fill (for the restorer, gen.py): where the speech is buried
(no response measured) inside 300-2700 Hz, the median of the response
measured there instead of none. The default target is the speech heard
over the noise (right for the mask model, which can't bring back what's
buried); a restorer taught that deletes the buried consonant bands.
"""
import json
import os
import sys
from multiprocessing import Pool

# one BLAS thread per worker: the pool already fills the cores (16
# workers each with a full BLAS pool put ~200 threads on 16 cores)
for _v in ("OPENBLAS_NUM_THREADS", "OMP_NUM_THREADS", "MKL_NUM_THREADS"):
    os.environ.setdefault(_v, "1")

import numpy as np  # noqa: E402
from scipy.ndimage import uniform_filter1d
from scipy.signal import resample_poly, welch

from score import lag, read_wav

MIN_SNR = -10.0
FILL = (300.0, 2700.0)


ENV_HOP = 16                                   # samples at 8 kS/s: 2 ms
ENV_REACH = 150                                # envelope lags tried each way (300 ms)


def envelopes(v: np.ndarray, fs: int) -> np.ndarray:
    """(8, frames): band log-power envelopes in 300-2700 Hz, each less its
    mean; phase-free, so a turning SSB phase doesn't matter."""
    from scipy.signal import stft
    f, _, Z = stft(v, fs, nperseg=128, noverlap=128 - ENV_HOP)
    p = np.abs(Z) ** 2
    edges = np.linspace(300, 2700, 9)
    e = np.stack([np.log10(p[(f >= a) & (f < b)].sum(0) + 1e-9) for a, b in zip(edges[:-1], edges[1:])])
    return e - e.mean(1, keepdims=True)


def own_lag(ex: np.ndarray, ec: np.ndarray, a: int, b: int, d: int) -> int:
    """The lag (samples) of the utterance in samples a..b of the clean
    source: the envelope correlation's best within ENV_REACH frames of
    the global lag d. All lags at once: the products by correlation, the
    windows' (mean-removed) norms from cumulative sums."""
    from scipy.signal import correlate
    fa, fb = a // ENV_HOP, b // ENV_HOP
    L = fb - fa
    seg = ec[:, fa:fb] - ec[:, fa:fb].mean(1, keepdims=True)
    lo = max(0, fa + d // ENV_HOP - ENV_REACH)
    hi = min(ex.shape[1] - L, fa + d // ENV_HOP + ENV_REACH)
    if L <= 0 or hi < lo:
        return d
    win = ex[:, lo:hi + L]
    # seg is zero-mean per band, so the window's mean drops out of the product
    num = sum(correlate(win[k], seg[k], mode="valid") for k in range(len(seg)))
    c1 = np.concatenate([np.zeros((len(win), 1)), np.cumsum(win, 1)], 1)
    c2 = np.concatenate([np.zeros((len(win), 1)), np.cumsum(win ** 2, 1)], 1)
    s1, s2 = c1[:, L:] - c1[:, :-L], c2[:, L:] - c2[:, :-L]
    wn = np.sqrt(np.maximum((s2 - s1 ** 2 / L).sum(0), 0.0))
    r = num / (wn * np.linalg.norm(seg) + 1e-12)
    return d + (lo + int(np.argmax(r)) - fa - d // ENV_HOP) * ENV_HOP


def one(args):
    db, path, fill, align = args
    k = path.rsplit("__", 1)[1][:-4]
    ann = json.load(open(f"{db}/annotations.json"))[k]
    x, fs = read_wav(path)
    c, _ = read_wav(f"{db}/clean/train/clean_{k}.wav")
    n = min(len(x), len(c))
    x, c = x[:n].astype(np.float64), c[:n].astype(np.float64)
    d = lag(c, x)
    spans = [tuple(map(int, s.split(":"))) for s in ann["activity"][0].split(", ")]
    if align:
        ex, ec = envelopes(x, fs), envelopes(c, fs)
        lags = [own_lag(ex, ec, max(0, a), min(n, b), d) if b - a >= fs // 2 else d for a, b in spans]
    else:
        lags = [d] * len(spans)
    ca = np.zeros(n)
    if align:
        # each utterance (and the silence up to halfway to its neighbours)
        # moved by its own lag
        cuts = [0] + [(spans[i][1] + spans[i + 1][0]) // 2 for i in range(len(spans) - 1)] + [n]
        for (u0, u1), L in zip(zip(cuts[:-1], cuts[1:]), lags):
            j0, j1 = max(0, u0 + L), min(n, u1 + L)
            if j1 > j0:
                ca[j0:j1] = c[j0 - L:j1 - L]
    elif d >= 0:
        ca[d:] = c[:n - d]
    else:
        ca[:n + d] = c[-d:]
    act = np.zeros(n, bool)
    for (a, b), L in zip(spans, lags):
        act[max(0, a + L):min(n, b + L)] = True
    if act.sum() < fs or (~act).sum() < fs:
        return None
    f, px = welch(x[act], fs, nperseg=256)
    _, pn = welch(x[~act], fs, nperseg=256)
    _, pc = welch(ca[act], fs, nperseg=256)
    band = (f >= 300) & (f <= 2700)
    snr = 10 * np.log10(max(px[band].sum() - pn[band].sum(), 1e-20) / pn[band].sum())
    if snr < MIN_SNR:
        return None
    # where the speech doesn't stand 10 % over the noise, the estimate is
    # noise itself: no response there
    heard = px - pn > 0.1 * pn
    h2 = np.where(heard, (px - pn) / (pc + 1e-20), 0.0)
    h2 = np.maximum(uniform_filter1d(h2, 5), 0.0)
    if fill:
        inb = (f >= FILL[0]) & (f <= FILL[1])
        meas = inb & heard
        if meas.any():
            h2 = np.where(inb & ~heard, np.median(h2[meas]), h2)
    # the response on the whole file's spectrum (zero phase)
    m = 1 << int(np.ceil(np.log2(2 * n)))
    fm = np.fft.rfftfreq(m, 1 / fs)
    h = np.sqrt(np.interp(fm, f, h2))
    s = np.fft.irfft(np.fft.rfft(ca, m) * h, m)[:n]
    x12, s12 = resample_poly(x, 3, 2), resample_poly(s, 3, 2)
    g = 0.05 / (np.sqrt(np.mean(x12 ** 2)) + 1e-12)
    q = lambda v: np.clip(np.round(v * g * 32768), -32768, 32767).astype(np.int16)  # noqa: E731
    return q(x12), q(s12), snr


def main():
    args = [v for v in sys.argv[1:] if v not in ("--fill", "--align")]
    fill, align = "--fill" in sys.argv, "--align" in sys.argv
    db, out = args[0], args[1]
    files = sorted(os.path.join(d, f) for d, _, fs in os.walk(f"{db}/noisy/train") for f in fs if f.endswith(".wav"))
    idx, pos, snrs = [], 0, []
    with open(out + ".x.i16", "wb") as fx, open(out + ".s.i16", "wb") as fs_, Pool(min(16, os.cpu_count() or 1)) as pool:
        for r in pool.imap(one, [(db, f, fill, align) for f in files], chunksize=8):
            if r is None:
                continue
            x, s, snr = r
            fx.write(x.tobytes())
            fs_.write(s.tobytes())
            idx.append((pos, len(x)))
            pos += len(x)
            snrs.append(snr)
    np.save(out + ".idx.npy", np.array(idx, np.int64))
    print(f"{len(idx)} of {len(files)} receptions kept, {pos / 12000 / 3600:.1f} h; "
          f"in-band SNR p10/50/90 {np.percentile(snrs, [10, 50, 90]).round(1)}")


if __name__ == "__main__":
    main()
