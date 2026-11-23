# SPDX-License-Identifier: GPL-3.0-or-later
"""The speech bank: every FLAC under a LibriSpeech directory, decoded
(the flac tool), resampled 16 → 12 kS/s and appended to one int16 file,
in a fixed order (by path). Utterances follow each other directly.

    python3 prep.py LibriSpeech/train-clean-100 speech.i16
"""
import os
import subprocess
import sys
from multiprocessing import Pool

import numpy as np
from scipy.signal import resample_poly


def load(path: str) -> np.ndarray:
    raw = subprocess.run(["flac", "-dcs", "--force-raw-format", "--endian=little", "--sign=signed", path],
                         check=True, capture_output=True).stdout
    x = np.frombuffer(raw, np.int16).astype(np.float32)
    y = resample_poly(x, 3, 4)
    return np.clip(np.round(y), -32768, 32767).astype(np.int16)


def main():
    src, out = sys.argv[1], sys.argv[2]
    files = sorted(os.path.join(d, f) for d, _, fs in os.walk(src) for f in fs if f.endswith(".flac"))
    total = 0
    with open(out, "wb") as fo, Pool(min(16, os.cpu_count() or 1)) as pool:
        for i, x in enumerate(pool.imap(load, files, chunksize=16)):
            fo.write(x.tobytes())
            total += len(x)
            if i % 2000 == 0:
                print(f"{i}/{len(files)} {total / 12000 / 3600:.1f} h", flush=True)
    print(f"{len(files)} files, {total / 12000 / 3600:.2f} h at 12 kS/s")


if __name__ == "__main__":
    main()
