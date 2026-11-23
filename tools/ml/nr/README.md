# Neural noise reduction: training

The SSB voice noise reduction planned in `doc/m8-browser-decoders.md`
§5.4 is trained here: a small mask-only network on 12 kS/s audio, for
the page's audio path. Nothing here is shipped; the trained weights
will be.

| file | what |
|---|---|
| `common.py` | STFT (causal, 256/128, square-root Hann), features, model (the Rust inference must match) |
| `data.py` | an example: LibriSpeech speech sent in SSB over an ionochannel HF path (fading, QRN, CW/RTTY/PSK31, carriers, man-made noise, mistuning, passband, AGC); the target is the same speech without the noise |
| `prep.py` | the speech bank: LibriSpeech FLAC → one 12 kS/s int16 file |
| `train.py` | training; validation by SI-SDR improvement at three SNR bands |

Speech: LibriSpeech (CC BY 4.0) `train-clean-100` for training,
`dev-clean` for validation (openslr.org resource 12; the EU mirror
openslr.elda.org is much faster). The channel comes from
[ionochannel](https://github.com/jeskko/ionochannel): installed, or a
checkout beside this repository (or at `IONOCHANNEL`).

```sh
cd tools/ml/nr
python3 prep.py LibriSpeech/train-clean-100 speech.i16     # ~100 h, 8.6 GB
python3 prep.py LibriSpeech/dev-clean speech-dev.i16
NR_SPEECH=speech.i16 NR_SPEECH_VAL=speech-dev.i16 python3 train.py --out /tmp/nr
```
