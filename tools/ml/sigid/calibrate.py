# SPDX-License-Identifier: GPL-3.0-or-later
"""Temperature for the identifier's probabilities (calibration):

    python3 calibrate.py run/best.pt [N]

On N fresh windows (a validation set the training didn't pick its best
checkpoint on), the T minimising the negative log-likelihood of
softmax(logits / T); prints the expected calibration error (15 bins)
before and after, and how often each confidence band is right. Export
with it: python3 export.py run/best.pt OUT --temperature T (folded into
the last layer).
"""
import sys

import numpy as np
import torch

from common import SigId, features
from data import fixed


def ece(p: np.ndarray, y: np.ndarray, bins=15) -> float:
    conf, pred = p.max(1), p.argmax(1)
    e = 0.0
    for lo in np.linspace(0, 1, bins, endpoint=False):
        m = (conf > lo) & (conf <= lo + 1 / bins)
        if m.any():
            e += m.mean() * abs((pred[m] == y[m]).mean() - conf[m].mean())
    return e


def main():
    ck = sys.argv[1]
    n = int(sys.argv[2]) if len(sys.argv) > 2 else 3000
    dev = "cuda" if torch.cuda.is_available() else "cpu"
    m = SigId().to(dev)
    m.load_state_dict(torch.load(ck, map_location=dev))
    m.eval()
    x, y, _ = fixed(n, 777)
    with torch.no_grad():
        z = torch.cat([m(*features(x[i:i + 64].to(dev))).cpu() for i in range(0, n, 64)])
    logt = torch.zeros(1, requires_grad=True)
    opt = torch.optim.LBFGS([logt], lr=0.1, max_iter=200)
    nll = torch.nn.CrossEntropyLoss()

    def step():
        opt.zero_grad()
        loss = nll(z / logt.exp(), y)
        loss.backward()
        return loss

    opt.step(step)
    t = float(logt.exp())
    yy = y.numpy()
    for name, tt in (("T=1", 1.0), (f"T={t:.3f}", t)):
        p = torch.softmax(z / tt, 1).numpy()
        conf, ok = p.max(1), p.argmax(1) == yy
        bands = " ".join(f"{lo:.1f}-{hi:.1f}: {100 * ok[(conf > lo) & (conf <= hi)].mean():.0f}% of {((conf > lo) & (conf <= hi)).sum()}"
                         for lo, hi in ((0, 0.6), (0.6, 0.8), (0.8, 0.95), (0.95, 1.0)))
        print(f"{name}: accuracy {100 * ok.mean():.1f} %, ECE {100 * ece(p, yy):.2f} %; right by confidence {bands}")
    print(f"temperature {t:.4f}")


if __name__ == "__main__":
    main()
