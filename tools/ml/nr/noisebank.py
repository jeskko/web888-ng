# SPDX-License-Identifier: GPL-3.0-or-later
"""Real HF noise for training: the pauses between utterances in the
Paderborn database's receptions (KiwiSDRs across Europe, LSB, 8 kS/s),
from a split that scoring doesn't use (train). Each reception's pauses
(the annotated speech, shifted by the reception's delay, kept out with
0.5 s to spare) are joined into one stream, resampled to 12 kS/s and
appended to one int16 file; the index (.npy) holds each stream's
start and length.

    python3 noisebank.py /data/nr/paderborn/ham_radio noise.i16
"""
import json
import os
import sys
from multiprocessing import Pool

import numpy as np
from scipy.signal import resample_poly

from score import lag, read_wav

SPARE = 0.5


def one(args):
    db, path = args
    k = path.rsplit("__", 1)[1][:-4]
    ann = json.load(open(f"{db}/annotations.json"))[k]
    x, fs = read_wav(path)
    clean, _ = read_wav(f"{db}/clean/train/clean_{k}.wav")
    d = lag(clean, x)
    keep = np.ones(len(x), bool)
    for s in ann["activity"][0].split(", "):
        a, b = map(int, s.split(":"))
        keep[max(0, a + d - int(SPARE * fs)):b + d + int(SPARE * fs)] = False
    # whole runs of at least 1 s
    edges = np.flatnonzero(np.diff(np.concatenate([[0], keep.astype(np.int8), [0]])))
    parts = [x[a:b] for a, b in zip(edges[::2], edges[1::2]) if b - a >= fs]
    if not parts:
        return np.zeros(0, np.int16)
    y = resample_poly(np.concatenate(parts), 3, 2)
    y = y / (np.sqrt(np.mean(y ** 2)) + 1e-12) * 0.05            # a common level; mixing sets the SNR
    return np.clip(np.round(y * 32768), -32768, 32767).astype(np.int16)


def main():
    db, out = sys.argv[1], sys.argv[2]
    files = sorted(os.path.join(d, f) for d, _, fs in os.walk(f"{db}/noisy/train") for f in fs if f.endswith(".wav"))
    idx, pos = [], 0
    with open(out, "wb") as fo, Pool(min(16, os.cpu_count() or 1)) as pool:
        for y in pool.imap(one, [(db, f) for f in files], chunksize=8):
            if len(y) >= 12000 * 4:
                fo.write(y.tobytes())
                idx.append((pos, len(y)))
                pos += len(y)
    np.save(out + ".idx.npy", np.array(idx, np.int64))
    print(f"{len(idx)} streams of {len(files)}, {pos / 12000 / 3600:.1f} h of noise")


if __name__ == "__main__":
    main()
