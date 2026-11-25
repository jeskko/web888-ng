# SPDX-License-Identifier: GPL-3.0-or-later
"""What a CW model keeps and takes, by SNR (in 2500 Hz, against the
wanted station's key-down power), on cwdata.py's examples (fixed seeds,
half with real noise): on the time-frequency cells where the tones stand
over the noise, the power gain (the closer to 0 dB the better) and the
share of those cells taken down by more than 6 dB (elements lost); on
the cells of noise alone (tones 20 dB under it, the passband only), the
power gain (the lower the better); and the SI-SDR improvement.

    NR_NOISE=noise.i16 python3 cwscore.py cw1/last.pt [cw2/last.pt] [--strength 1,2]
"""
import argparse
import os

import numpy as np
import torch

import cwdata
from common import enhance, load, stft
from train import si_sdr

BANDS = [(-18.0, -10.0), (-10.0, -3.0), (-3.0, 5.0), (5.0, 20.0)]


def db(v):
    return 10 * np.log10(max(float(v), 1e-20))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("ckpts", nargs="+")
    ap.add_argument("--n", type=int, default=256, help="examples per SNR band")
    ap.add_argument("--strength", default="1", help="strengths to score (over 1: the gain raised to it)")
    a = ap.parse_args()
    dev = "cuda"
    nets = {os.path.basename(os.path.dirname(c)) or c: load(torch.load(c, map_location=dev), dev) for c in a.ckpts}
    print(f"{'SNR dB':>10} {'model':>12} {'str':>4}  {'tones dB':>8} {'lost %':>6}  {'noise dB':>8}  {'SI-SDR +dB':>10}")
    for i, band in enumerate(BANDS):
        x, y, _ = cwdata.fixed(a.n, 2000 + i, band)
        x, y = x.to(dev), y.to(dev)
        X, S = stft(x), stft(y)
        PX, PS, PN = X.abs() ** 2, S.abs() ** 2, (X - S).abs() ** 2
        tone = PS > PN
        gap = (PS < 0.01 * PN) & (PN > 1e-3 * PN.mean(dim=(1, 2), keepdim=True))
        print(f"{band[0]:+4.0f}..{band[1]:+3.0f}  ({int(tone.sum())} tone, {int(gap.sum())} noise cells)")
        for name, net in nets.items():
            for s in (float(v) for v in a.strength.split(",")):
                floor, power = (1 - s, 1.0) if s <= 1 else (0.0, s)
                with torch.no_grad():
                    e = torch.cat([enhance(net, x[j:j + 64], floor, power) for j in range(0, len(x), 64)])
                PE = stft(e).abs() ** 2
                kept = db(PE[tone].sum() / PX[tone].sum())
                lost = 100 * float((PE[tone] < 0.25 * PS[tone]).float().mean())
                taken = db(PE[gap].sum() / PX[gap].sum())
                imp = float(np.nanmean((si_sdr(e, y) - si_sdr(x, y)).cpu().numpy()))
                print(f"{'':>10} {name:>12} {s:4.1f}  {kept:8.1f} {lost:6.1f}  {taken:8.1f}  {imp:+10.2f}")


if __name__ == "__main__":
    main()
