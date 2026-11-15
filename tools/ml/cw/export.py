# SPDX-License-Identifier: GPL-3.0-or-later
"""Export a trained CwNet for crates/cw (src/net.rs):

    python3 export.py run1/best.pt ../../../crates/cw/model

writes cwnet.bin (the weights as f16) and cwnet-ref.bin (a reference: a
channel's samples and the model's log-probabilities for them, computed
with the f16-rounded weights, for the Rust test).

cwnet.bin: b"W8CWNET1", u32 tensor count, then per tensor: u16 name
length, name (UTF-8), u8 dims, u32 each dim, f16 values (little endian).
cwnet-ref.bin: b"W8CWREF1", u32 n, n complex samples (f32 re, im), u32 T,
u32 C, T x C f32 log-probabilities.
"""
import random
import struct
import sys

import numpy as np
import torch

from common import CwNet, features, fold_bn
from synth import batch


def main():
    ck, out = sys.argv[1], sys.argv[2]
    m = CwNet()
    # a checkpoint without batch norm (an early run) loads too
    m.load_state_dict(torch.load(ck, map_location="cpu"), strict=False)
    m.eval()
    sd = {k: v.detach().half().float() for k, v in fold_bn(m.state_dict()).items() if not k.endswith("num_batches_tracked")}
    # what the Rust side will compute with: the folded f16 weights, no batch norm
    m.n1 = m.n2 = m.n3 = torch.nn.Identity()
    m.load_state_dict(sd)
    with open(f"{out}/cwnet.bin", "wb") as f:
        f.write(b"W8CWNET1")
        f.write(struct.pack("<I", len(sd)))
        for k, v in sd.items():
            name = k.encode()
            f.write(struct.pack("<H", len(name)) + name)
            f.write(struct.pack("<B", v.dim()))
            for d in v.shape:
                f.write(struct.pack("<I", d))
            f.write(v.numpy().astype("<f2").tobytes())
    x, _ = batch(1, random.Random(7), "cpu", n=1000, snr=(5.0, 6.0))
    with torch.no_grad():
        lp = m(features(x))[0]
    with open(f"{out}/cwnet-ref.bin", "wb") as f:
        f.write(b"W8CWREF1")
        xs = x[0].numpy()
        f.write(struct.pack("<I", len(xs)))
        f.write(np.stack([xs.real, xs.imag], 1).astype("<f4").tobytes())
        f.write(struct.pack("<II", *lp.shape))
        f.write(lp.numpy().astype("<f4").tobytes())
    n = sum(v.numel() for v in sd.values())
    print(f"{n} parameters -> {out}/cwnet.bin; reference {tuple(lp.shape)}")


if __name__ == "__main__":
    main()
