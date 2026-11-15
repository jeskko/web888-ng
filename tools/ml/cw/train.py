# SPDX-License-Identifier: GPL-3.0-or-later
"""Train the neural CW decoder (CTC) on synthetic data made on the fly.

    python3 train.py --steps 30000 --out run1

Checkpoints (best validation character error rate) go to OUT/best.pt; the
log to OUT/log.txt. export.py writes the weights crates/cw embeds.
"""
import argparse
import os
import random
import time

import torch
import torch.nn.functional as F

from common import CLASSES, CwNet, features, greedy, out_len
from synth import batch


def cer(ref: str, hyp: str) -> float:
    a, b = ref.replace(" ", ""), hyp.replace(" ", "")
    d = list(range(len(b) + 1))
    for i, ca in enumerate(a, 1):
        prev, d[0] = d[:], i
        for j, cb in enumerate(b, 1):
            d[j] = min(prev[j] + 1, d[j - 1] + 1, prev[j - 1] + (ca != cb))
    return d[-1] / max(1, len(a))


def val_set(device, n=256):
    """Fixed validation batches at SNR bands (2500 Hz)."""
    out = []
    for lo, hi in ((10, 20), (0, 10), (-6, 0), (-12, -6)):
        r = random.Random(1000 + lo)
        x, lab = batch(n, r, device, snr=(lo, hi))
        out.append((f"{lo:+d}..{hi:+d} dB", features(x), lab))
    return out


@torch.no_grad()
def evaluate(model, vs):
    model.eval()
    res = {}
    for name, f, lab in vs:
        hyp = []
        for i in range(0, f.shape[0], 128):
            hyp += greedy(model(f[i:i + 128]))
        refs = ["".join(CLASSES[c] for c in l) for l in lab]
        res[name] = sum(cer(r, h) for r, h in zip(refs, hyp)) / len(refs)
    model.train()
    return res


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--steps", type=int, default=30000)
    ap.add_argument("--batch", type=int, default=128)
    ap.add_argument("--lr", type=float, default=3e-3)
    ap.add_argument("--out", default="run")
    ap.add_argument("--resume", default="")
    ap.add_argument("--seed", type=int, default=1)
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    dev = "cuda" if torch.cuda.is_available() else "cpu"
    torch.manual_seed(a.seed)
    model = CwNet().to(dev)
    if a.resume:
        model.load_state_dict(torch.load(a.resume, map_location=dev))
    opt = torch.optim.AdamW(model.parameters(), lr=a.lr, weight_decay=1e-4)
    sched = torch.optim.lr_scheduler.OneCycleLR(opt, max_lr=a.lr, total_steps=a.steps, pct_start=0.05)
    vs = val_set(dev)
    r = random.Random(a.seed)
    best = 1e9
    log = open(os.path.join(a.out, "log.txt"), "a")
    t0 = time.time()
    for step in range(1, a.steps + 1):
        # curriculum: clean signals first, the full SNR range by 30 %
        k = min(1.0, step / (0.3 * a.steps))
        x, lab = batch(a.batch, r, dev, snr=(10.0 - 22.0 * k, 20.0))
        f = features(x)
        lp = model(f)                                        # (B, T, C)
        T = lp.shape[1]
        assert T == out_len(f.shape[-1])
        tgt = torch.tensor([c for l in lab for c in l], dtype=torch.long)
        tl = torch.tensor([len(l) for l in lab], dtype=torch.long)
        il = torch.full((len(lab),), T, dtype=torch.long)
        loss = F.ctc_loss(lp.transpose(0, 1), tgt, il, tl, blank=0, zero_infinity=True)
        opt.zero_grad()
        loss.backward()
        torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
        opt.step()
        sched.step()
        if step % 500 == 0 or step == a.steps:
            res = evaluate(model, vs)
            mean = sum(res.values()) / len(res)
            line = (f"step {step} loss {loss.item():.3f} lr {sched.get_last_lr()[0]:.2e} {time.time() - t0:.0f}s "
                    + " ".join(f"[{k}] {100 * v:.1f}%" for k, v in res.items()) + f" mean {100 * mean:.1f}%")
            print(line, flush=True)
            log.write(line + "\n")
            log.flush()
            if mean < best:
                best = mean
                torch.save(model.state_dict(), os.path.join(a.out, "best.pt"))
            torch.save(model.state_dict(), os.path.join(a.out, "last.pt"))


if __name__ == "__main__":
    main()
