# Signal identifier: training

The signal identifier in `crates/sigid` (M8 item 8,
`doc/m8-browser-decoders.md` §5.3) is trained here on synthetic signals
made on the fly. Nothing here is shipped; the trained weights are, as
`crates/sigid/model/sigid.bin` (f16, embedded in `sigid.wasm`).

| file | what |
|---|---|
| `modes.py` | clean signals of the 25 classes: noise, carrier, CW, 2-FSK (RTTY, SITOR-B, ASCII, DWD), PSK31/63/125, FT8 and FT4 (with their neighbours), WSPR, ALE, STANAG 4285, MIL-STD-188-110A, SSTV (13 modes), HF fax, SSB voice, AM (speech or music; sometimes one sideband, as heard in SSB), local interference (carriers with phase-noise or bursting skirts, pairs and combs, noise bands still, drifting or wandering, mains hum; regular wideband ticks or pulsing skirts), OTH radar (FMCW sweeps, pulse trains), HFDL (1800 Bd bursts: prekey, preamble, data and probe frames) |
| | since 2026-11-23 also: Olivia/Contestia (4-64 tones, 125-2000 Hz), MFSK8/16/32/64, DominoEX/THOR (18 tones, incremental keying), Feld Hell (on-off pixels, 245 slots/s), PACTOR I-III in ARQ (1.25 s cycle, the other side's control bursts), multi-tone modems (MT63, the 39-tone modem with its pilot, OFDM data as VARA), 3G ALE (2400 Bd 8-PSK bursts) |
| `onair.py` | on-air speech: SSB clips cut from a phone segment of a wideband recording, where a model says SSB (`SIGID_ONAIR`: 40 % of the speech in training) |
| `voices.py` | the speech bank (espeak-ng, ~40 languages and voice variants) for voice and AM |
| `data.py` | an example: a signal off centre, fading (QSB or auroral flutter) and multipath from ionochannel, neighbours of other classes, a birdie, impulse noise, white noise at −6..+30 dB in 2500 Hz, the receiver's passband edges, LSB, any level |
| `common.py` | features and model (must match `crates/sigid/src/lib.rs` exactly) |
| `train.py` | training (worker processes generate, the GPU computes the features); a fixed validation set; `--eval` prints the confusion matrix |
| `real.py` | real signals: waterfalls of a recording (`survey`), the confusion on labelled windows of recordings (`eval`) |
| `calibrate.py` | the temperature that calibrates the probabilities (on a fresh validation set), the calibration error before and after |
| `export.py` | `sigid.bin` and `sigid-ref.bin` (a reference input and output for the Rust test); `--temperature T` folds the calibration into the last layer |

Content is random where it doesn't identify a mode (FT8's data tones, a
modem's data symbols, a picture's pixels) and exact where it does (Costas
arrays, frame and probe structure, scramblers, baud rates, shifts, tone
spacings, SSTV line timings). Needs PyTorch, scipy and espeak-ng. The channel (fading, multipath) comes from
[ionochannel](https://github.com/jeskko/ionochannel) at the commit pinned
in the workspace `Cargo.toml`: installed (`pip install
"ionochannel @ git+https://github.com/jeskko/ionochannel@<rev>"`) or a
checkout beside this repository (or at `IONOCHANNEL`).

```sh
cd tools/ml/sigid
python3 voices.py voices.npz 600                                        # ~20 s
python3 onair.py REC.cs8 4e6 7100000 7060000 7200000 lsb MODEL.pt onair.npz  # optional: on-air speech
SIGID_VOICES=voices.npz SIGID_ONAIR=onair.npz python3 train.py --steps 30000 --out /tmp/sid   # ~2 h 10 on 16 threads + RTX 5070 (data-bound)
SIGID_VOICES=voices.npz SIGID_ONAIR=onair.npz python3 train.py --resume /tmp/sid/best.pt --steps 10000 --lr 5e-4 --out /tmp/sid2   # a fine-tune after changing a generator
# classes added at the end of modes.CLASSES: --resume keeps the old ones' rows of the last layer
python3 real.py eval /tmp/sid/best.pt LABELS.txt                        # real recordings
python3 calibrate.py /tmp/sid/best.pt                                    # prints the temperature T
python3 export.py /tmp/sid/best.pt ../../../crates/sigid/model --temperature T
cargo test -p web888-sigid --release                                    # the Rust inference matches PyTorch
cargo run --release -p web888-sigid --example scanwav FILE.wav          # the passband scanner on 12 kHz audio
```

`LABELS.txt` (for `real.py eval`): one station a line, `file rate
centre_hz class hz t0 [t1 step]`, e.g. a HackRF recording
`/data/iq/2026-10-23_8600k_4000k_40m30m_12min.cs8 4000000 8600000 fsk
7646000 10 700 30` (windows every 30 s).
