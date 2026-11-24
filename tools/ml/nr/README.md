# Neural noise reduction: training

The SSB voice noise reduction planned in `doc/m8-browser-decoders.md`
§5.4 is trained here: a small mask-only network on 12 kS/s audio, for
the page's audio path. Nothing here is shipped; the trained weights
will be.

| file | what |
|---|---|
| `common.py` | STFT (causal, 256/128, square-root Hann), features, model (the Rust inference must match) |
| `data.py` | examples: LibriSpeech speech through a ham transmitter's chain (EQ, compressor, clipper), sent in SSB over an ionochannel HF path (fading, QRN, CW/RTTY/PSK31, carriers, man-made noise, mistuning, passband, AGC), with real KiwiSDR noise on part of them; the target is the same speech without the noise. `batch()` makes whole batches on the GPU |
| `noisebank.py` | real HF noise: the pauses of the Paderborn database's train receptions, 96 h |
| `score.py` | Paderborn eval receptions raw and processed: word errors by Whisper, Vosk or Parakeet, STOI and ESTOI |
| `stoi.py` | STOI and ESTOI (from the papers) |
| `prep.py` | the speech bank: LibriSpeech FLAC → one 12 kS/s int16 file |
| `train.py` | training; validation by SI-SDR improvement at four SNR bands |
| `export.py` | `nrnet.bin` and `nrnet-ref.bin` for `crates/nr` (or a weights file for the page's model selector: list it in `nr-models.json` in the UI directory) |
| `attn.py` | the gain in speech frames and in pauses, per band, on real receptions |

Speech: LibriSpeech (CC BY 4.0) `train-clean-100` for training,
`dev-clean` for validation (openslr.org resource 12; the EU mirror
openslr.elda.org is much faster). The channel comes from
[ionochannel](https://github.com/jeskko/ionochannel): installed, or a
checkout beside this repository (or at `IONOCHANNEL`).

Real noise and scoring: the Paderborn HF speech database (Zenodo
4247491, CC BY 4.0, 11 GB). Vosk (`vosk`, en-us 0.22 lgraph) and
Parakeet TDT 0.6B v2 (`sherpa-onnx`, int8) for `score.py --asr` go in a
venv with `--system-site-packages`; the models under `NR_ASR_MODELS`.
Run heavy jobs with `nice`.

```sh
cd tools/ml/nr
python3 prep.py LibriSpeech/train-clean-100 speech.i16     # ~100 h, 8.6 GB
python3 prep.py LibriSpeech/dev-clean speech-dev.i16
python3 noisebank.py ham_radio noise.i16                    # Paderborn train pauses
# the shipped weights: run 6 (30k steps from scratch)
NR_SPEECH=speech.i16 NR_SPEECH_VAL=speech-dev.i16 NR_NOISE=noise.i16 nice -n 10 \
  python3 train.py --steps 30000 --hidden 128 --bands --pitch --residual 10,15 --lost 4 \
  --real-noise 0.5 --out /tmp/nr6
python3 export.py /tmp/nr6/last.pt ../../../crates/nr/model
cargo test -p web888-nr                                    # the Rust stream matches PyTorch
venv/bin/python score.py ham_radio /tmp/nr6/last.pt --asr whisper,parakeet,vosk --floors 0,0.2
python3 attn.py ham_radio /tmp/nr5/last.pt /tmp/nr6/last.pt   # voice kept, noise taken, per band
```
