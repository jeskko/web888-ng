# SPDX-License-Identifier: GPL-3.0-or-later
"""Train the SSB voice noise reduction (common.py) on data.py's examples.

Loss on power-law compressed spectra (0.3), magnitude and complex, plus
a penalty for speech taken away (output below the target), so leftover
noise costs less than lost speech. Validation: fixed sets at three SNR
bands, scored by SI-SDR improvement over the input.

    NR_SPEECH=speech.i16 NR_SPEECH_VAL=speech-dev.i16 nice -n 10 python3 train.py --out /tmp/nr

Batches are made on the GPU by worker processes (data.batch, ~0.6 s of
mostly single-threaded work each, so 8 workers keep up); --cpu-data
makes examples one at a time on CPU workers instead.
"""
import argparse
import os
import time

import numpy as np
import torch

from common import Net, enhance, istft, stft
from data import GpuStream, Stream, fixed

BANDS = {"-20..-10": (-20.0, -10.0), "-10..-3": (-10.0, -3.0), "-3..5": (-3.0, 5.0), "5..15": (5.0, 15.0)}
C = 0.3
LOST = 1.0                       # the weight of speech taken away (--lost)


def comp(X):
    m = X.abs().clamp_min(1e-8)
    return m ** C, X / m ** (1 - C)


def loss_fn(net, x, y):
    X, Y = stft(x), stft(y)
    g = net.gains(x, X)
    E = X * g
    me, ce = comp(E)
    my, cy = comp(Y)
    mag = ((me - my) ** 2).mean()
    cpx = (ce - cy).abs().pow(2).mean()
    lost = torch.relu(my - me).pow(2).mean()
    return mag + 0.3 * cpx + LOST * lost


def loss_sdw(net, x, s, keep, mu):
    """Speech-distortion weighted: the gain on the speech alone against 1
    and on the noise alone against `keep`, both on power-law compressed
    magnitudes, the noise term weighted by mu (below 1: speech kept before
    noise taken). Per cell the best gain is then (in the compressed
    domain) (|S|^2c + mu keep^c |N|^2c) / (|S|^2c + mu |N|^2c): it leans
    to keeping the voice where the voice is weak against the noise."""
    X, S = stft(x), stft(s)
    N = X - S
    g = net.gains(x, X) ** C
    ms, mn = S.abs() ** (2 * C), N.abs() ** (2 * C)
    kc = keep[:, :, None] ** C
    return (ms * (g - 1) ** 2).mean() + mu * (mn * (g - kc) ** 2).mean()


def si_sdr(e, y):
    """(B, n) estimate and target → dB per row; rows with a silent target
    are NaN."""
    a = (e * y).sum(-1, keepdim=True) / (y * y).sum(-1, keepdim=True).clamp_min(1e-12)
    t = a * y
    r = (t * t).sum(-1) / ((e - t) ** 2).sum(-1).clamp_min(1e-12)
    out = 10 * torch.log10(r.clamp_min(1e-12))
    return torch.where((y * y).sum(-1) > 1e-9, out, torch.full_like(out, float("nan")))


@torch.no_grad()
def evaluate(net, sets, dev):
    net.eval()
    res = {}
    for k, (x, y, _) in sets.items():
        x, y = x.to(dev), y.to(dev)
        e = torch.cat([enhance(net, x[i:i + 64]) for i in range(0, len(x), 64)])
        res[k] = (float(np.nanmean((si_sdr(e, y) - si_sdr(x, y)).cpu().numpy())),
                  float(loss_fn(net, x, y)))
    net.train()
    return res


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--steps", type=int, default=30000)
    ap.add_argument("--batch", type=int, default=64)
    ap.add_argument("--lr", type=float, default=1e-3)
    ap.add_argument("--hidden", type=int, default=96)
    ap.add_argument("--workers", type=int, default=8)
    ap.add_argument("--cpu-data", action="store_true", help="examples one at a time on CPU workers (slower)")
    ap.add_argument("--lost", type=float, default=1.0, help="weight of speech taken away")
    ap.add_argument("--snr", default="-20,15", help="training SNR range in 2500 Hz (dB)")
    ap.add_argument("--real-noise", type=float, default=0.0,
                    help="share of batch groups with real HF noise (noisebank.py, NR_NOISE) instead of synthetic")
    ap.add_argument("--bands", action="store_true", help="gains on 32 smooth bands, not per bin")
    ap.add_argument("--pitch", action="store_true", help="pitch (autocorrelation) features as input too")
    ap.add_argument("--residual", default="", help="dB range of noise kept in the target, e.g. 10,15 (none: clean)")
    ap.add_argument("--sdw", type=float, default=0.0,
                    help="speech-distortion weighted loss with this noise weight mu (0: the plain loss)")
    ap.add_argument("--resume")
    ap.add_argument("--eval", action="store_true")
    ap.add_argument("--out", default="/tmp/nr")
    a = ap.parse_args()
    global LOST
    LOST = a.lost
    snr = tuple(float(v) for v in a.snr.split(","))
    residual = tuple(float(v) for v in a.residual.split(",")) if a.residual else None
    dev = "cuda" if torch.cuda.is_available() else "cpu"
    os.makedirs(a.out, exist_ok=True)
    net = Net(a.hidden, a.bands, a.pitch).to(dev)
    if a.resume:
        net.load_state_dict(torch.load(a.resume, map_location=dev)["model"])
    print(f"{sum(p.numel() for p in net.parameters())} weights")
    t0 = time.time()
    sets = {k: fixed(128, 1000 + i, b) for i, (k, b) in enumerate(BANDS.items())}
    print(f"validation sets in {time.time() - t0:.0f} s")
    if a.eval:
        print(evaluate(net, sets, dev))
        return
    if a.cpu_data:
        dl = torch.utils.data.DataLoader(Stream(int(time.time())), batch_size=a.batch, num_workers=a.workers,
                                         persistent_workers=True, prefetch_factor=4)
    else:
        # whole batches made on the GPU by each worker (data.batch)
        dl = torch.utils.data.DataLoader(GpuStream(int(time.time()), a.batch, snr, a.real_noise, residual, a.sdw > 0),
                                         batch_size=None,
                                         num_workers=a.workers, multiprocessing_context="spawn",
                                         persistent_workers=True, prefetch_factor=2)
    opt = torch.optim.AdamW(net.parameters(), a.lr, weight_decay=1e-4)
    sched = torch.optim.lr_scheduler.OneCycleLR(opt, a.lr, total_steps=a.steps, pct_start=0.05)
    best, t0, run = -1e9, time.time(), 0.0
    for step, b in enumerate(dl, 1):
        b = [v.to(dev, non_blocking=True) for v in b]
        loss = loss_sdw(net, *b, a.sdw) if a.sdw > 0 else loss_fn(net, *b)
        opt.zero_grad()
        loss.backward()
        torch.nn.utils.clip_grad_norm_(net.parameters(), 1.0)
        opt.step()
        sched.step()
        run = 0.98 * run + 0.02 * loss.item() if step > 1 else loss.item()
        if step % 500 == 0 or step == a.steps:
            r = evaluate(net, sets, dev)
            score = sum(v[0] for v in r.values()) / len(r)
            print(f"{step:6d} loss {run:.4f} " + " ".join(f"{k} {v[0]:+.2f} dB" for k, v in r.items())
                  + f"  {time.time() - t0:.0f} s", flush=True)
            ck = {"model": net.state_dict(), "hidden": a.hidden, "step": step, "val": r, "args": vars(a)}
            torch.save(ck, f"{a.out}/last.pt")
            if score > best:
                best = score
                torch.save(ck, f"{a.out}/best.pt")
        if step >= a.steps:
            break


if __name__ == "__main__":
    main()
