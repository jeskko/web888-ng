# Signal identifier: training

The signal identifier in `crates/sigid` (M8 item 8,
`doc/m8-browser-decoders.md` §5.3) is trained here on synthetic signals
made on the fly. Nothing here is shipped; the trained weights are, as
`crates/sigid/model/sigid.bin` (f16, embedded in `sigid.wasm`).

| file | what |
|---|---|
| `modes.py` | clean signals of the 15 classes: noise, carrier, CW, 2-FSK (RTTY, SITOR-B, ASCII, DWD), PSK31/63/125, FT8 and FT4 (with their neighbours), WSPR, ALE, STANAG 4285, MIL-STD-188-110A, SSTV (13 modes), HF fax, SSB voice, AM (speech or music; sometimes one sideband, as heard in SSB) |
| `voices.py` | the speech bank (espeak-ng, ~40 languages and voice variants) for voice and AM |
| `data.py` | an example: a signal off centre, fading (QSB or auroral flutter), multipath, neighbours of other classes, a birdie, impulse noise, white noise at −6..+30 dB in 2500 Hz, the receiver's passband edges, LSB, any level |
| `common.py` | features and model (must match `crates/sigid/src/lib.rs` exactly) |
| `train.py` | training (worker processes generate, the GPU computes the features); a fixed validation set; `--eval` prints the confusion matrix |
| `real.py` | real signals: waterfalls of a recording (`survey`), the confusion on labelled windows of recordings (`eval`) |
| `export.py` | `sigid.bin` and `sigid-ref.bin` (a reference input and output for the Rust test) |

Content is random where it doesn't identify a mode (FT8's data tones, a
modem's data symbols, a picture's pixels) and exact where it does (Costas
arrays, frame and probe structure, scramblers, baud rates, shifts, tone
spacings, SSTV line timings). Needs PyTorch, scipy and espeak-ng.

```sh
cd tools/ml/sigid
python3 voices.py voices.npz 600                                        # ~20 s
SIGID_VOICES=voices.npz python3 train.py --steps 20000 --out /tmp/sid   # ~1 h 40 on 16 threads + RTX 5070 (data-bound)
python3 real.py eval /tmp/sid/best.pt LABELS.txt                        # real recordings
python3 export.py /tmp/sid/best.pt ../../../crates/sigid/model
cargo test -p web888-sigid --release                                    # the Rust inference matches PyTorch
cargo run --release -p web888-sigid --example scanwav FILE.wav          # the passband scanner on 12 kHz audio
```

`LABELS.txt` (for `real.py eval`): one station a line, `file rate
centre_hz class hz t0 [t1 step]`, e.g. a HackRF recording
`/data/iq/2026-10-23_8600k_4000k_40m30m_12min.cs8 4000000 8600000 fsk
7646000 10 700 30` (windows every 30 s).
