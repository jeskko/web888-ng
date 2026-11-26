# SPDX-License-Identifier: GPL-3.0-or-later
"""Where a restorer (gen.py) differs from the mask model on real receptions
(Paderborn eval): band power against the mask output in speech and in
pauses, and the band envelopes' spread in speech against the clean
source's (smeared speech: less; a restorer 20 dB too quiet shows here,
not in SI-SDR).

    python3 gendiag.py ham_radio MASK.pt GEN.pt 40
"""
import json, os, sys
import numpy as np, torch
from scipy.signal import resample_poly, stft as sstft
from score import read_wav, lag
from common import enhance, load
import gen as g

db, mask_ck, gen_ck, n = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
dev = "cuda"
net = load(torch.load(mask_ck, map_location=dev), dev)
gen = g.load_gen(torch.load(gen_ck, map_location=dev), dev)
ann = json.load(open(f"{db}/annotations.json"))
files = sorted(os.path.join(d, f) for d, _, fs in os.walk(f"{db}/noisy/eval") for f in fs if f.endswith(".wav"))
rng = np.random.default_rng(1)
files = [files[i] for i in sorted(rng.choice(len(files), n, replace=False))]
BANDS = [(150, 500), (500, 1000), (1000, 2000), (2000, 3500)]
def bandpow(x, fs=12000):
    f, t, Z = sstft(x, fs, nperseg=256, noverlap=128)
    P = np.abs(Z) ** 2
    return np.stack([P[(f >= a) & (f < b)].sum(0) for a, b in BANDS]), t
acc = {c: {"sp": [], "pa": [], "dyn": []} for c in ["raw", "nn0", "gen1", "gen2"]}
cdyn = []
for fi, path in enumerate(files):
    k = path.rsplit("__", 1)[1][:-4]
    x8, fs = read_wav(path)
    clean, _ = read_wav(f"{db}/clean/eval/clean_{k}.wav")
    d = lag(clean, x8)
    x = resample_poly(x8, 3, 2).astype(np.float32)
    xt = torch.from_numpy(x[: len(x) // 128 * 128])[None].to(dev)
    with torch.no_grad():
        outs = {"raw": x[: xt.shape[1]], "nn0": enhance(net, xt, 0.0)[0].cpu().numpy()}
        for s in (1, 2):
            outs[f"gen{s}"] = g.restore(gen, net, xt, s, seed=fi)[0].cpu().numpy()
    # speech frames from the annotation (8 kS/s sample spans)
    spans = [tuple(map(int, s.split(":"))) for s in ann[k]["activity"][0].split(", ")]
    Pm, t = bandpow(outs["nn0"])
    sp = np.zeros(len(t), bool)
    for s0, s1 in spans:
        sp |= (t >= s0 / fs) & (t < s1 / fs)
    c12 = resample_poly(clean, 3, 2)
    dd = int(round(d * 1.5))
    cal = np.zeros_like(outs["nn0"]); m = min(len(cal) - max(dd, 0), len(c12))
    cal[max(dd, 0):max(dd, 0) + m] = c12[:m]
    Pc, _ = bandpow(cal)
    lg = lambda P: 10 * np.log10(P + 1e-12)
    cdyn.append(lg(Pc[:, sp]).std(1))
    for c, y in outs.items():
        P, _ = bandpow(y)
        acc[c]["sp"].append(lg(P[:, sp]).mean(1) - lg(Pm[:, sp]).mean(1))
        acc[c]["pa"].append(lg(P[:, ~sp]).mean(1) - lg(Pm[:, ~sp]).mean(1))
        acc[c]["dyn"].append(lg(P[:, sp]).std(1))
print("bands (Hz):", BANDS)
print("clean: envelope std in speech (dB):", np.round(np.mean(cdyn, 0), 1))
for c, a in acc.items():
    print(f"{c:5} speech vs mask {np.round(np.mean(a['sp'], 0), 1)}  pauses vs mask {np.round(np.mean(a['pa'], 0), 1)}  envelope std {np.round(np.mean(a['dyn'], 0), 1)}")
