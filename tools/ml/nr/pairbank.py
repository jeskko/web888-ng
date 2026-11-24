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

    python3 pairbank.py /data/nr/paderborn/ham_radio pairs
    # -> pairs.x.i16, pairs.s.i16, pairs.idx.npy
"""
import json
import os
import sys
from multiprocessing import Pool

import numpy as np
from scipy.ndimage import uniform_filter1d
from scipy.signal import resample_poly, welch

from score import lag, read_wav

MIN_SNR = -10.0


def one(args):
    db, path = args
    k = path.rsplit("__", 1)[1][:-4]
    ann = json.load(open(f"{db}/annotations.json"))[k]
    x, fs = read_wav(path)
    c, _ = read_wav(f"{db}/clean/train/clean_{k}.wav")
    n = min(len(x), len(c))
    x, c = x[:n].astype(np.float64), c[:n].astype(np.float64)
    d = lag(c, x)
    ca = np.zeros(n)
    if d >= 0:
        ca[d:] = c[:n - d]
    else:
        ca[:n + d] = c[-d:]
    act = np.zeros(n, bool)
    for s in ann["activity"][0].split(", "):
        a, b = map(int, s.split(":"))
        act[max(0, a + d):min(n, b + d)] = True
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
    h2 = np.where(px - pn > 0.1 * pn, (px - pn) / (pc + 1e-20), 0.0)
    h2 = np.maximum(uniform_filter1d(h2, 5), 0.0)
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
    db, out = sys.argv[1], sys.argv[2]
    files = sorted(os.path.join(d, f) for d, _, fs in os.walk(f"{db}/noisy/train") for f in fs if f.endswith(".wav"))
    idx, pos, snrs = [], 0, []
    with open(out + ".x.i16", "wb") as fx, open(out + ".s.i16", "wb") as fs_, Pool(min(16, os.cpu_count() or 1)) as pool:
        for r in pool.imap(one, [(db, f) for f in files], chunksize=8):
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
