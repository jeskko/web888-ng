# SPDX-License-Identifier: GPL-3.0-or-later
"""Train the signal identifier on synthetic examples made on the fly
(data.py), worker processes generating, the GPU computing features.

    SIGID_VOICES=voices.npz python3 train.py --steps 20000 --out run1

OUT/best.pt (best validation accuracy), OUT/log.txt; --eval prints the
confusion matrix and accuracy by SNR of --resume.
"""
import argparse
import os
import time

# one thread each: the data workers fill the cores (numpy's pools oversubscribe them)
for v in ("OMP_NUM_THREADS", "OPENBLAS_NUM_THREADS", "MKL_NUM_THREADS"):
    os.environ.setdefault(v, "1")

import numpy as np
import torch
import torch.nn.functional as F

from common import SigId, features
from data import Stream, fixed
from modes import CLASSES


@torch.no_grad()
def predict(model, x, dev, bs=128):
    model.eval()
    out = []
    for i in range(0, len(x), bs):
        out.append(model(*features(x[i:i + bs].to(dev))).argmax(1).cpu())
    model.train()
    return torch.cat(out)


def report(pred, lab, snr):
    k = len(CLASSES)
    cm = np.zeros((k, k), int)
    for p, l in zip(pred.numpy(), lab.numpy()):
        cm[l, p] += 1
    print("confusion (rows: true):")
    print(" " * 12 + " ".join(f"{c[:5]:>5}" for c in CLASSES))
    for i, c in enumerate(CLASSES):
        print(f"{c:<11} " + " ".join(f"{v:5d}" for v in cm[i]) + f"   {100 * cm[i, i] / cm[i].sum():5.1f} %")
    ok = (pred == lab).numpy()
    for lo, hi in ((-6, 0), (0, 6), (6, 15), (15, 30)):
        m = (snr >= lo) & (snr < hi)
        print(f"SNR {lo:+3d}..{hi:+3d} dB: {100 * ok[m].mean():5.1f} % of {m.sum()}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--steps", type=int, default=20000)
    ap.add_argument("--batch", type=int, default=128)
    ap.add_argument("--lr", type=float, default=2e-3)
    ap.add_argument("--out", default="run")
    ap.add_argument("--resume", default="")
    ap.add_argument("--workers", type=int, default=14)
    ap.add_argument("--eval", action="store_true")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    dev = "cuda" if torch.cuda.is_available() else "cpu"
    torch.manual_seed(1)
    model = SigId().to(dev)
    if a.resume:
        model.load_state_dict(torch.load(a.resume, map_location=dev))
    vx, vl, vs = fixed(1500, 12345)
    if a.eval:
        report(predict(model, vx, dev), vl, vs)
        return
    dl = torch.utils.data.DataLoader(Stream(1), batch_size=a.batch, num_workers=a.workers,
                                     persistent_workers=True, prefetch_factor=4)
    opt = torch.optim.AdamW(model.parameters(), lr=a.lr, weight_decay=1e-4)
    sched = torch.optim.lr_scheduler.OneCycleLR(opt, max_lr=a.lr, total_steps=a.steps, pct_start=0.05)
    log = open(os.path.join(a.out, "log.txt"), "a")
    best, t0 = 0.0, time.time()
    for step, (x, y) in enumerate(dl, 1):
        logits = model(*features(x.to(dev, non_blocking=True)))
        loss = F.cross_entropy(logits, y.to(dev), label_smoothing=0.05)
        opt.zero_grad()
        loss.backward()
        opt.step()
        sched.step()
        if step % 500 == 0 or step == a.steps:
            acc = (predict(model, vx, dev) == vl).float().mean().item()
            line = f"step {step} loss {loss.item():.3f} {time.time() - t0:.0f}s val {100 * acc:.1f}%"
            print(line, flush=True)
            log.write(line + "\n")
            log.flush()
            if acc > best:
                best = acc
                torch.save(model.state_dict(), os.path.join(a.out, "best.pt"))
            torch.save(model.state_dict(), os.path.join(a.out, "last.pt"))
        if step == a.steps:
            break
    model.load_state_dict(torch.load(os.path.join(a.out, "best.pt"), map_location=dev))
    report(predict(model, vx, dev), vl, vs)


if __name__ == "__main__":
    main()
