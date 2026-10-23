#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Cut USB audio at dial frequencies from an IQ recording into 12 kHz WAVs,
one per UTC slot, named like WSJT-X (YYMMDD_HHMMSS.wav), for scoring the
decoders against jt9/wsprd (doc/m4-skimmers.md §3).

Each slot is cut exactly: the slot's IQ is transformed, the band from the
dial up to 6 kHz is kept, and an inverse real FFT gives the audio.

  slotcut.py REC.cs8 --rate 4e6 --centre 8.6e6 --start UTC_SECONDS \
      --out DIR ft8:7.074e6 ft4:7.0475e6 wspr:7.0386e6 ...
"""
import argparse, math, os, sys, wave
import numpy as np

SLOT = {"ft8": 15.0, "ft4": 7.5, "wspr": 120.0}
AUDIO = 12000


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("rec")
    ap.add_argument("--rate", type=float, required=True)
    ap.add_argument("--centre", type=float, required=True)
    ap.add_argument("--start", type=float, required=True, help="UTC of the first sample (s)")
    ap.add_argument("--format", default="cs8", choices=["cs8", "cs16", "cf32"])
    ap.add_argument("--out", required=True)
    ap.add_argument("--max-slots", type=int, default=0)
    ap.add_argument("dials", nargs="+", help="mode:dial_hz")
    a = ap.parse_args()

    dt = {"cs8": np.int8, "cs16": np.int16, "cf32": np.float32}[a.format]
    raw = np.memmap(a.rec, dtype=dt, mode="r")
    nsamp = raw.size // 2
    end = a.start + nsamp / a.rate
    for spec in a.dials:
        mode, dial = spec.split(":")
        dial = float(dial)
        period = SLOT[mode]
        off = dial - a.centre
        if abs(off) + 6000 > a.rate / 2:
            sys.exit(f"{spec}: outside the recording")
        outdir = os.path.join(a.out, f"{mode}_{dial / 1e3:.1f}k")
        os.makedirs(outdir, exist_ok=True)
        t = math.ceil(a.start / period) * period
        n_out = int(round(period * AUDIO))
        count = 0
        while t + period <= end:
            i0 = int(round((t - a.start) * a.rate))
            n = int(round(period * a.rate))
            iq = raw[2 * i0 : 2 * (i0 + n)].astype(np.float32)
            x = iq[0::2] + 1j * iq[1::2]
            X = np.fft.fft(x)
            df = a.rate / n
            k0 = int(round(off / df)) % n
            nb = n_out // 2 + 1
            Y = np.take(X, np.arange(k0, k0 + nb), mode="wrap").copy()
            Y[-1] = 0
            y = np.fft.irfft(Y, n=n_out) * (n_out / n)
            y -= y.mean()
            # Same headroom for every slot of a band: scale from the first.
            if count == 0:
                scale = 0.1 * 32767 / max(np.sqrt(np.mean(y * y)), 1e-9)
            pcm = np.clip(y * scale, -32767, 32767).astype("<i2")
            name = __import__("time").strftime("%y%m%d_%H%M%S", __import__("time").gmtime(t))
            if period == 7.5 and (t % 15) != 0:
                name += ".5"
            with wave.open(os.path.join(outdir, name + ".wav"), "wb") as w:
                w.setnchannels(1)
                w.setsampwidth(2)
                w.setframerate(AUDIO)
                w.writeframes(pcm.tobytes())
            count += 1
            t += period
            if a.max_slots and count >= a.max_slots:
                break
        print(f"{spec}: {count} slots -> {outdir}")


if __name__ == "__main__":
    main()
