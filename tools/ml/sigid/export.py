# SPDX-License-Identifier: GPL-3.0-or-later
"""Export a trained SigId for crates/sigid:

    python3 export.py run1/best.pt ../../../crates/sigid/model [--temperature T]

writes sigid.bin (batch norm folded into the convolutions, weights as
f16; the format of crates/cw's cwnet.bin with magic b"W8SIGID1") and
sigid-ref.bin (a reference for the Rust test: b"W8SIDREF", u32 n, n
complex samples as f32 re, im, u32 C, C f32 logits computed with the
folded f16 weights). With --temperature T (calibrate.py) the last layer
is divided by T: the logits come out calibrated.
"""
import struct
import sys

import numpy as np
import torch

from common import SigId, features
from data import example


def fold(sd: dict, seqs) -> dict:
    """Each Conv followed by a BatchNorm in the named Sequentials: folded."""
    out = dict(sd)
    for conv, bn in seqs:
        s = sd[f"{bn}.weight"] / torch.sqrt(sd[f"{bn}.running_var"] + 1e-5)
        w = sd[f"{conv}.weight"]
        out[f"{conv}.weight"] = w * s.view(-1, *([1] * (w.dim() - 1)))
        out[f"{conv}.bias"] = (sd[f"{conv}.bias"] - sd[f"{bn}.running_mean"]) * s + sd[f"{bn}.bias"]
        for k in ("weight", "bias", "running_mean", "running_var", "num_batches_tracked"):
            out.pop(f"{bn}.{k}", None)
    return out


PAIRS = ([(f"spec.{i}.0", f"spec.{i}.1") for i in range(4)] + [("spec.4", "spec.5")]
         + [(f"fine.{i}.0", f"fine.{i}.1") for i in range(3)] + [(f"ac.{i}.0", f"ac.{i}.1") for i in range(2)])


def main():
    ck, out = sys.argv[1], sys.argv[2]
    temp = float(sys.argv[sys.argv.index("--temperature") + 1]) if "--temperature" in sys.argv else 1.0
    m = SigId()
    sd0 = torch.load(ck, map_location="cpu")
    for k in ("head.3.weight", "head.3.bias"):
        sd0[k] = sd0[k] / temp
    m.load_state_dict(sd0)
    m.eval()
    sd = {k: v.detach().half().float() for k, v in fold(m.state_dict(), PAIRS).items()}
    # the reference with the folded f16 weights and no batch norm, as the Rust side computes
    for _, name in PAIRS:
        parent, _, idx = name.rpartition(".")
        mod = m
        for part in parent.split("."):
            mod = mod[int(part)] if part.isdigit() else getattr(mod, part)
        mod[int(idx)] = torch.nn.Identity()
    m.load_state_dict(sd)
    with open(f"{out}/sigid.bin", "wb") as f:
        f.write(b"W8SIGID1")
        f.write(struct.pack("<I", len(sd)))
        for k, v in sd.items():
            name = k.encode()
            f.write(struct.pack("<H", len(name)) + name)
            f.write(struct.pack("<B", v.dim()))
            for d in v.shape:
                f.write(struct.pack("<I", d))
            f.write(v.numpy().astype("<f2").tobytes())
    x, c, _ = example(np.random.default_rng(7), label=9, snr=(10.0, 10.0))
    xt = torch.from_numpy(x[None])
    with torch.no_grad():
        logits = m(*features(xt))[0]
    with open(f"{out}/sigid-ref.bin", "wb") as f:
        f.write(b"W8SIDREF")
        f.write(struct.pack("<I", len(x)))
        f.write(np.stack([x.real, x.imag], 1).astype("<f4").tobytes())
        f.write(struct.pack("<I", len(logits)))
        f.write(logits.numpy().astype("<f4").tobytes())
    n = sum(v.numel() for v in sd.values())
    print(f"{n} parameters -> {out}/sigid.bin; reference class {c}, logits argmax {int(logits.argmax())}")


if __name__ == "__main__":
    main()
