# web888-client

The reference client for web888d's listener protocol
([`doc/api.md`](../../doc/api.md)): one Python file, standard library
only (Python 3.8 or later), so it can be copied anywhere. It is also the
test of the document: it uses only what the document describes, and CI
runs it against the simulator (`test.sh`).

```sh
c=tools/web888-client/web888_client.py
python3 $c info http://receiver:8073/
python3 $c audio http://receiver:8073/ --hz 7074000 --mode usb --seconds 60 --out ft8.wav
python3 $c audio http://receiver:8073/ --hz 77500 --mode iq --seconds 120 --sigmf dcf77
python3 $c wide http://receiver:8073/ --hz 7030000 --seconds 30 --sigmf cw40
python3 $c waterfall http://receiver:8073/ --center 7100000 --decim 64 --fps 1 --seconds 600 > wf.jsonl
python3 $c spots http://receiver:8073/ > spots.jsonl
```

| command | what it does |
|---|---|
| `info` | `/api/version`, then the session's `hello`: protocol, features, the receiver, the lease |
| `audio` | tunes (`--hz`, `--mode`, `--lo`/`--hi`, `--rate`, `--gain`, `--no-agc`), waits for a receiver, records PCM16 to a WAV (`--out`) and/or SigMF (`--sigmf BASE`): `ri16_le`, or `ci16_le` in mode `iq` |
| `wide` | one channelizer bin (60 kS/s complex, clean within ±21 kHz of its centre) as SigMF `cf32_le`, each frame's scale applied |
| `waterfall` | lines as JSON: `t` (UTC s, the end of the line), `f0` (first bin's centre, Hz), `df`, `db` (absolute dBFS) |
| `spots` | the skimmers' spots as JSON lines: the last 30 min, then new ones |

**Times.** When the server has the `timestamps` feature (protocol 3),
the client asks for them, and each SigMF capture carries
`core:datetime`: the antenna time of its first sample (doc/api.md §8.5).
A new capture starts after a gap or a jump in time. The global
`web888:timebase` says where the receiver's clock came from (`ntp`,
`holdover`, `none`), its uncertainty (s) and the ADC clock's error
(ppm) when known.

**Access.** With a listener password set, or for limits beyond the
anonymous ones, an API token (made in the admin page's Passwords tab)
goes in `--token` or `WEB888_TOKEN`; it is sent as
`Authorization: Bearer`. A `read` token can't open `/ws` (`listen` or
`admin` can).

**Ends.** The client exits with the server's reason when it is closed:
a limit (4001-4003), an unsupported protocol (4004), a kick or a revoked
token (1008), or a refusal at admission (1013, after `busy`).
