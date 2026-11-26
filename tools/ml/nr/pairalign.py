# SPDX-License-Identifier: GPL-3.0-or-later
"""How well the real pairs line up (pairbank.py --align), per utterance:

    python3 pairalign.py ham_radio 60

Phase-free alignment per utterance: band log-power envelopes (8 bands
in 300-2700 Hz, 2 ms hop) of the clean source and the reception,
correlated over lags; the utterance's own lag against the reception's
global one (pairbank.py's), and the envelope correlation at each."""
import json, os, sys
import numpy as np
from scipy.signal import stft
from score import lag, read_wav
db, n = sys.argv[1], int(sys.argv[2])
ann = json.load(open(f"{db}/annotations.json"))
files = sorted(os.path.join(d, f) for d, _, fs in os.walk(f"{db}/noisy/train") for f in fs if f.endswith(".wav"))
rng = np.random.default_rng(0)
files = [files[i] for i in sorted(rng.choice(len(files), n, replace=False))]
HOP = 16
def env(v, fs=8000):
    f, _, Z = stft(v, fs, nperseg=128, noverlap=128 - HOP)
    P = np.abs(Z) ** 2
    edges = np.linspace(300, 2700, 9)
    E = np.stack([np.log10(P[(f >= a) & (f < b)].sum(0) + 1e-9) for a, b in zip(edges[:-1], edges[1:])])
    return E - E.mean(1, keepdims=True)
def ecorr(Ex, Ec, i0, n):
    if i0 < 0 or i0 + n > Ex.shape[1]: return np.nan
    a, b = Ex[:, i0:i0 + n], Ec
    a = a - a.mean(1, keepdims=True); b = b - b.mean(1, keepdims=True)
    return float((a * b).sum() / (np.linalg.norm(a) * np.linalg.norm(b) + 1e-12))
rows, recs = [], []
for path in files:
    k = path.rsplit("__", 1)[1][:-4]
    x, fs = read_wav(path); c, _ = read_wav(f"{db}/clean/train/clean_{k}.wav")
    m = min(len(x), len(c)); x, c = x[:m].astype(float), c[:m].astype(float)
    d = lag(c, x)
    Ex, Ec = env(x), env(c)
    own = []
    for s in ann[k]["activity"][0].split(", "):
        a, b = map(int, s.split(":")); fa, fb = a // HOP, min(b, m) // HOP
        if fb - fa < 400: continue
        g = (d // HOP)
        cs = [ecorr(Ex, Ec[:, fa:fb], fa + g + dl, fb - fa) for dl in range(-150, 151)]   # +-300 ms
        cs = np.nan_to_num(np.array(cs), nan=-1)
        i = int(np.argmax(cs)); dl = i - 150
        rows.append((dl * HOP, cs[150], cs[i]))
        own.append(dl * HOP)
    if len(own) > 1: recs.append((k, d, own))
o = np.array(rows)
print(f"{len(o)} utterances in {len(files)} receptions (lags in samples @8k; 16 = 2 ms)")
print("own - global: p10/50/90", np.percentile(o[:, 0], [10, 50, 90]), " |.|>=16 (2 ms):", np.mean(np.abs(o[:, 0]) >= 16).round(2), " >=80 (10 ms):", np.mean(np.abs(o[:, 0]) >= 80).round(2))
print("envelope corr at global p10/50/90", np.percentile(o[:, 1], [10, 50, 90]).round(2), " at own", np.percentile(o[:, 2], [10, 50, 90]).round(2))
spread = [np.ptp(r[2]) for r in recs]
print("spread of own lags within a reception p50/90/max", np.percentile(spread, [50, 90, 100]))
for r in recs[:8]: print("  ", r[0], "global", r[1], "own - global", r[2])
