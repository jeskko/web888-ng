# Neural CW decoder: training

The neural CW decoder in `crates/cw/src/net.rs` (M8 item 7,
`doc/m8-browser-decoders.md` §5) is trained here, on synthetic CW made on
the fly. Nothing here is shipped; the trained weights are, as
`crates/cw/model/cwnet.bin` (f16, embedded in `cw.wasm`).

| file | what |
|---|---|
| `common.py` | alphabet, features, model (must match `net.rs` exactly) |
| `synth.py` | training data: text, keying, channel (fading, interference, impulse noise, white noise) |
| `train.py` | CTC training with a fixed validation set at four SNR bands |
| `export.py` | `cwnet.bin` and `cwnet-ref.bin` (a reference input and output for `net.rs`'s test) |

Needs PyTorch (Arch: `python-pytorch-cuda`) and a GPU (CPU works, slowly).

```sh
cd tools/ml/cw
python3 train.py --steps 30000 --lr 1e-3 --out /tmp/cwrun   # ~40 min on an RTX 5070
python3 export.py /tmp/cwrun/best.pt ../../../crates/cw/model
cargo test -p web888-cw --release net                      # the Rust inference matches PyTorch
cargo run --release -p web888-cw --example cer             # classical vs neural
```

The training synthesiser is independent of the Rust one
(`crates/cw/src/synth.rs`) that the comparison uses, so the network is
not scored on the generator it learnt from.
