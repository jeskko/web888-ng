# SPDX-License-Identifier: GPL-3.0-or-later
"""Export a trained model for crates/nr:

    python3 export.py /tmp/nr/best.pt ../../../crates/nr/model

writes nrnet.bin (the weights as f16) and nrnet-ref.bin (a reference: 2 s
of input audio and the model's output for it, computed with the
f16-rounded weights, for the Rust test).

nrnet.bin: b"W8NRNET1", u32 tensor count, then per tensor: u16 name
length, name (UTF-8), u8 dims, u32 each dim, f16 values (little endian).
nrnet-ref.bin: b"W8NRREF1", u32 n, n f32 input samples, n f32 output.
"""
import struct
import sys

import numpy as np
import torch

from common import enhance, load
from data import Bank, example


def main():
    ck, out = sys.argv[1], sys.argv[2]
    c = torch.load(ck, map_location="cpu")
    m = load(c, "cpu")
    sd = {k: v.half().float() for k, v in c["model"].items()}
    m.load_state_dict(sd)
    with open(f"{out}/nrnet.bin", "wb") as f:
        f.write(b"W8NRNET1")
        f.write(struct.pack("<I", len(sd)))
        for k, v in sd.items():
            name = k.encode()
            f.write(struct.pack("<H", len(name)) + name)
            f.write(struct.pack("<B", v.dim()))
            for d in v.shape:
                f.write(struct.pack("<I", d))
            f.write(v.numpy().astype("<f2").tobytes())
    x, _, _ = example(np.random.default_rng(7), Bank(), snr=(3.0, 4.0))
    x = torch.from_numpy(x[:24064])[None]           # 2 s, a whole number of hops
    with torch.no_grad():
        y = enhance(m, x)[0]
    with open(f"{out}/nrnet-ref.bin", "wb") as f:
        f.write(b"W8NRREF1")
        f.write(struct.pack("<I", x.shape[1]))
        f.write(x[0].numpy().astype("<f4").tobytes())
        f.write(y.numpy().astype("<f4").tobytes())
    n = sum(v.numel() for v in sd.values())
    print(f"{n} parameters -> {out}/nrnet.bin ({2 * n // 1024} KiB)")


if __name__ == "__main__":
    main()
