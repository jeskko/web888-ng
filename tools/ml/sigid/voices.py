# SPDX-License-Identifier: GPL-3.0-or-later
"""The speech bank for SSB voice and AM broadcast examples: espeak-ng
utterances in many languages and voice variants, random speed, pitch and
text, as 6 kS/s audio (float16) in one .npz.

    python3 voices.py OUT.npz [N]
"""
import os
import random
import subprocess
import sys
import tempfile

import numpy as np
from scipy.signal import resample_poly

LANGS = ("en-us en-gb de fr-fr es it pt ru pl cs sk fi sv nb da nl hu ro bg uk el tr ar fa hi ja cmn ko "
         "id vi lt lv et hr sr sl ca eu he").split()
VARIANTS = ["", "+m1", "+m2", "+m3", "+m4", "+m5", "+m6", "+m7", "+f1", "+f2", "+f3", "+f4", "+f5",
            "+croak", "+klatt", "+klatt2", "+klatt3"]
WORDS = ("this is a test station the weather is fine today thank you for the call my name is and "
         "the antenna is a dipole we are listening on the frequency roger copy all over please "
         "repeat your signal is five and nine here in the city news at the hour today the minister "
         "said that the government will meet tomorrow in the capital music and sports report "
         "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima mike november "
         "oscar papa quebec romeo sierra tango uniform victor whiskey x-ray yankee zulu one two "
         "three four five six seven eight nine zero").split()


def text(r: random.Random) -> str:
    return " ".join(r.choice(WORDS) for _ in range(r.randrange(12, 30))) + "."


def main():
    out, n = sys.argv[1], int(sys.argv[2]) if len(sys.argv) > 2 else 600
    r = random.Random(1)
    clips = []
    with tempfile.TemporaryDirectory() as d:
        w = os.path.join(d, "x.wav")
        while len(clips) < n:
            v = r.choice(LANGS) + r.choice(VARIANTS)
            cmd = ["espeak-ng", "-v", v, "-s", str(r.randrange(120, 220)), "-p", str(r.randrange(15, 85)),
                   "-g", str(r.randrange(0, 8)), "-w", w, text(r)]
            if subprocess.run(cmd, capture_output=True).returncode != 0:
                continue
            import wave
            with wave.open(w) as f:
                fs = f.getframerate()
                x = np.frombuffer(f.readframes(f.getnframes()), np.int16).astype(np.float32) / 32768
            y = resample_poly(x, 6000 // np.gcd(6000, fs), fs // np.gcd(6000, fs))
            if len(y) < 6000 * 3:
                continue
            clips.append((y / (np.abs(y).max() + 1e-9)).astype(np.float16))
    lens = np.array([len(c) for c in clips])
    np.savez(out, audio=np.concatenate(clips), lens=lens)
    print(f"{len(clips)} clips, {lens.sum() / 6000 / 60:.1f} min")


if __name__ == "__main__":
    main()
