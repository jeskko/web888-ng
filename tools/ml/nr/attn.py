# SPDX-License-Identifier: GPL-3.0-or-later
"""What a model takes away on real receptions, band by band: on the
Paderborn eval receptions (the annotated speech, shifted by each
reception's delay), the power-weighted gain in speech frames (the voice
kept, with its noise) and in the pauses (the noise taken), the median
over receptions; the closer the first to 0 dB and the lower the second,
the better.

    python3 attn.py /data/nr/paderborn/ham_radio run5/last.pt run6/last.pt [--files 60] [--floor 0]
"""
import argparse
import json
import os

import numpy as np
import torch
from scipy.signal import resample_poly

from common import FS, HOP, NFFT, enhance, load, stft
from score import lag, read_wav

EDGES = [0, 300, 600, 1000, 1500, 2000, 2500, 3000, 4000]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("db")
    ap.add_argument("ckpts", nargs="+")
    ap.add_argument("--files", type=int, default=60)
    ap.add_argument("--floor", type=float, default=0.0)
    a = ap.parse_args()
    ann = json.load(open(f"{a.db}/annotations.json"))
    files = sorted(os.path.join(d, f) for d, _, fs in os.walk(f"{a.db}/noisy/eval") for f in fs if f.endswith(".wav"))
    rng = np.random.default_rng(1)
    files = [files[i] for i in sorted(rng.choice(len(files), min(a.files, len(files)), replace=False))]
    dev = "cuda" if torch.cuda.is_available() else "cpu"
    nets = {c: load(torch.load(c, map_location=dev), dev) for c in a.ckpts}
    fr = np.fft.rfftfreq(NFFT, 1 / FS)
    res = {c: ([], []) for c in a.ckpts}
    for p in files:
        k = p.rsplit("__", 1)[1][:-4]
        x8, fs = read_wav(p)
        clean, _ = read_wav(f"{a.db}/clean/eval/clean_{k}.wav")
        d = lag(clean, x8)
        x = resample_poly(x8, 3, 2).astype(np.float32)
        x = x[: len(x) // HOP * HOP]
        act = np.zeros(len(x), bool)
        for s in ann[k]["activity"][0].split(", "):
            s0, s1 = map(int, s.split(":"))
            act[int((s0 + d) * 1.5):int((s1 + d) * 1.5)] = True
        xt = torch.from_numpy(x)[None].to(dev)
        P = (stft(xt).abs() ** 2)[0].cpu().numpy()
        T = P.shape[0]
        fa = np.array([act[min(len(x) - 1, t * HOP)] for t in range(T)])
        for c, net in nets.items():
            with torch.no_grad():
                Y = (stft(enhance(net, xt, a.floor)).abs() ** 2)[0].cpu().numpy()

            def gain(m):
                return [10 * np.log10(Y[m][:, (fr >= lo) & (fr < hi)].sum() / (P[m][:, (fr >= lo) & (fr < hi)].sum() + 1e-12) + 1e-12)
                        for lo, hi in zip(EDGES[:-1], EDGES[1:])]

            res[c][0].append(gain(fa))
            res[c][1].append(gain(~fa))
    print(f"{len(files)} receptions, floor {a.floor}; gain in dB (power), median over receptions")
    print(f"{'band Hz':24}" + "".join(f"{lo:>6}" for lo in EDGES[:-1]))
    for c in a.ckpts:
        name = os.path.basename(os.path.dirname(c)) or c
        print(f"{name + ' speech':24}" + "".join(f"{v:6.1f}" for v in np.median(res[c][0], 0)))
        print(f"{name + ' pauses':24}" + "".join(f"{v:6.1f}" for v in np.median(res[c][1], 0)))


if __name__ == "__main__":
    main()
