# SPDX-License-Identifier: GPL-3.0-or-later
"""On-air speech for the voice bank: SSB stations of a phone segment in a
wideband recording (cs8, as from hackrf_transfer), every 4 s, as the
app's scanner finds regions; a region a model calls SSB (p >= PMIN) and
at least 800 Hz wide becomes a 4 s clip of audio (the sideband turned
upright, 100-2900 Hz) at FS. The generators add their own passband,
noise and fading to it as they do to the espeak bank.

    python3 onair.py REC.cs8 RATE CENTRE F_LO F_HI usb|lsb MODEL.pt OUT.npz [PMIN]

MODEL.pt: a checkpoint with 15 classes (before interference, radar and
HFDL) or the current ones. OUT.npz: audio (float32), lens (as voices.npz).
"""
import sys

import numpy as np
import torch
from scipy.ndimage import percentile_filter, uniform_filter1d
from scipy.signal import butter, sosfilt

import common
import modes

FS, N = 6000, 24576


def main():
    path, rate, centre, f_lo, f_hi, side, mpath, out = sys.argv[1:9]
    rate, centre, f_lo, f_hi = float(rate), float(centre), float(f_lo), float(f_hi)
    pmin = float(sys.argv[9]) if len(sys.argv) > 9 else 0.9
    dev = "cuda" if torch.cuda.is_available() else "cpu"
    sd = torch.load(mpath, map_location=dev)
    k = sd["head.3.weight"].shape[0]
    model = common.SigId(n_classes=k).to(dev)
    model.load_state_dict(sd)
    model.eval()
    ssb = modes.CLASSES.index("ssb")
    L = int(round(N / FS * rate))
    total = np.memmap(path, np.int8, mode="r").shape[0] // 2
    clips = []
    t = np.arange(N) / FS
    sos = butter(6, [100, 2900], "bandpass", fs=FS, output="sos")
    for t0 in np.arange(0, total / rate - N / FS, 4.0):
        raw = np.fromfile(path, np.int8, count=2 * L, offset=2 * int(t0 * rate))
        x = torch.from_numpy(raw.astype(np.float32).reshape(-1, 2)).to(dev)
        X = torch.fft.fft(torch.complex(x[:, 0], x[:, 1]) / 128.0)
        P = torch.fft.fftshift(X.abs() ** 2).reshape(-1, 100).sum(1).cpu().numpy()
        df = rate / L * 100
        f = (np.arange(len(P)) - len(P) / 2) * df
        sm = uniform_filter1d(10 * np.log10(P + 1e-12), 5)
        floor = percentile_filter(sm, 30, size=int(20000 / df))
        use = (f + centre >= f_lo) & (f + centre <= f_hi) & (np.abs(f) > 2000)
        above = (sm > floor + 4) & use
        regs, i = [], 0
        while i < len(above):
            if not above[i]:
                i += 1
                continue
            j = i
            while True:
                m = j + 1
                while m < len(above) and not above[m] and m - j <= 8:
                    m += 1
                if m < len(above) and above[m] and m - j <= 9:
                    j = m
                else:
                    break
            if (sm[i:j + 1] - floor[i:j + 1]).max() >= 10 and (j - i + 1) * df >= 800:
                regs.append(0.5 * (f[i] + f[j]))
            i = j + 1
        if not regs:
            continue
        ys = []
        for c in regs:
            k0 = int(round(c / (rate / L)))
            idx = (k0 + np.arange(-N // 2, N // 2)) % L
            Y = X[torch.from_numpy(idx).to(dev)]
            ys.append(torch.fft.ifft(torch.fft.ifftshift(Y)) * (N / L))
        ys = torch.stack(ys)
        with torch.no_grad():
            p = torch.softmax(model(*common.features(ys)), 1)[:, ssb].cpu().numpy()
        for y, q in zip(ys.cpu().numpy(), p):
            if q < pmin:
                continue
            # the voice band to 0-3 kHz upright: LSB is mirrored
            z = np.conj(y) if side == "lsb" else y
            Z = np.fft.fft(z)
            Z[np.abs(np.fft.fftfreq(N, 1 / FS)) > 1450] = 0
            a = np.real(np.fft.ifft(Z) * np.exp(1j * 2 * np.pi * 1500 * t))
            a = sosfilt(sos, a)
            clips.append((a / (np.abs(a).max() + 1e-9)).astype(np.float32))
        print(f"{t0:.0f} s: {len(regs)} regions, {len(clips)} clips", flush=True)
    np.savez(out, audio=np.concatenate(clips), lens=np.array([len(c) for c in clips]))


if __name__ == "__main__":
    main()
