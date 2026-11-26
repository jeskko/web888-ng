# Neural noise reduction: training

The SSB voice and CW noise reduction planned in `doc/m8-browser-decoders.md`
§5.4 is trained here: a small mask-only network on 12 kS/s audio, for
the page's audio path. Nothing here is shipped; the trained weights
will be.

| file | what |
|---|---|
| `common.py` | STFT (causal, 256/128, square-root Hann), features, model (the Rust inference must match) |
| `data.py` | examples: LibriSpeech speech through a ham transmitter's chain (EQ, compressor, clipper), sent in SSB over an ionochannel HF path (fading, QRN, CW/RTTY/PSK31, carriers, man-made noise, mistuning, passband, AGC), with real KiwiSDR noise on part of them; the target is the same speech without the noise. `batch()` makes whole batches on the GPU |
| `cwdata.py` | CW examples: keyed stations (the CW decoder's synthesiser: text, speed, jitter) with their own fading, QRN or real noise, man-made noise, a CW or SSB filter, the AGC; the target is every keyed tone with the noise 10-15 dB down (`train.py --cw`) |
| `amdata.py` | AM broadcast examples: speech and music through broadcast processing, AM, an ionochannel path, noise and neighbours, a DSB or SSB filter, the envelope or synchronous detector; the target is the same without the noise, the noise 10-15 dB down (`train.py --am`) |
| `musicbank.py` | MUSAN's music at 12 kS/s, a held-out split by artist, a licence list per track (`--strict`: CC BY, CC0, public domain only) |
| `cwscore.py` | a CW model on cwdata's examples by SNR: tones kept, tone cells lost, noise taken, SI-SDR |
| `noisebank.py` | real HF noise: the pauses of the Paderborn database's train receptions, 96 h |
| `pairbank.py` | real training pairs: Paderborn train receptions and the speech inside them (the clean source, delayed and through each reception's estimated channel response), 100 h; `train.py --pairs` mixes them into the batches |
| `score.py` | Paderborn eval receptions raw and processed: word errors by Whisper, Vosk or Parakeet, STOI and ESTOI |
| `stoi.py` | STOI and ESTOI (from the papers) |
| `prep.py` | the speech bank: LibriSpeech FLAC → one 12 kS/s int16 file |
| `train.py` | training; validation by SI-SDR improvement at four SNR bands |
| `export.py` | `nrnet.bin` and `nrnet-ref.bin` for `crates/nr` (or a weights file for the page's model selector: list it in `nr-models.json` in the UI directory) |
| `gen.py` | the generative voice restorer ("Gen" in the page, may invent): a conditional flow from the frozen mask model's output to clean speech in the same STFT; training and its validation (SI-SDR of the mask and of 1 and 2 flow steps) |
| `gendiag.py` | a restorer against the mask on real receptions: band power in speech and pauses, envelope spread against the clean source (level losses SI-SDR doesn't see) |
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
# the shipped weights: run 7 (45k steps from scratch; run 6 was the same at
# --hidden 128 and 30k steps)
NR_SPEECH=speech.i16 NR_SPEECH_VAL=speech-dev.i16 NR_NOISE=noise.i16 nice -n 10 \
  python3 train.py --steps 45000 --hidden 192 --bands --pitch --residual 10,15 --lost 4 \
  --real-noise 0.5 --out /tmp/nr7
python3 export.py /tmp/nr7/last.pt ../../../crates/nr/model
cargo test -p web888-nr                                    # the Rust stream matches PyTorch
venv/bin/python score.py ham_radio /tmp/nr6/last.pt --asr whisper,parakeet,vosk --floors 0,0.2
python3 attn.py ham_radio /tmp/nr5/last.pt /tmp/nr6/last.pt   # voice kept, noise taken, per band
# the CW model (cwdata.py's examples; the shipped weights: cw1)
NR_NOISE=noise.i16 nice -n 10 python3 train.py --cw --hidden 64 --residual 10,15 --lost 4 \
  --real-noise 0.5 --workers 4 --out /tmp/cw1
python3 export.py /tmp/cw1/last.pt ../../../crates/nr/model --cw   # nrnet-cw.bin
NR_NOISE=noise.i16 python3 cwscore.py /tmp/cw1/last.pt --strength 1,2   # tones kept, noise taken, by SNR
# the AM model (amdata.py's examples; the shipped weights: am1); MUSAN from
# openslr.org/17 (11 GB), its music part extracted
python3 musicbank.py musan/music music.i16 music-val.i16   # 37 h + 5.8 h
NR_SPEECH=speech.i16 NR_SPEECH_VAL=speech-dev.i16 NR_MUSIC=music.i16 NR_MUSIC_VAL=music-val.i16 \
  NR_NOISE=noise.i16 nice -n 10 python3 train.py --am --steps 45000 --hidden 192 --pitch \
  --residual 10,15 --lost 4 --real-noise 0.5 --workers 4 --out /tmp/am1
NR_SPEECH=speech.i16 python3 export.py /tmp/am1/last.pt ../../../crates/nr/model --am   # nrnet-am.bin
# the generative voice restorer (the shipped weights: gen1, on run 7's mask)
NR_SPEECH=speech.i16 NR_SPEECH_VAL=speech-dev.i16 NR_NOISE=noise.i16 nice -n 10 \
  python3 gen.py --mask /tmp/nr7/last.pt --out /tmp/gen1                # 60k steps, 2.5 h
# (gen1 fails on real receptions: 23 dB too quiet, words lost; being
# replaced by --mag, the magnitude only with the noisy phase, and
# --pairs 0.5, half of each batch real receptions, NR_PAIRS=pairs)
NR_SPEECH=speech.i16 python3 export.py /tmp/gen1/last.pt ../../../crates/nr/model --gen /tmp/nr7/last.pt   # nrgen.bin
venv/bin/python score.py ham_radio /tmp/nr7/last.pt --asr whisper,parakeet --floors 0 \
  --gen /tmp/gen1/last.pt --gen-steps 1,2      # invented words: substitutions and insertions
```
