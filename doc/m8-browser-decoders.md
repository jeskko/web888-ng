# M8: more decoders in the browser (plan B)

Status: in progress (planned 2026-11-15). Done: item 1 (CW; `crates/cw`,
`crates/cw-wasm`, `ui/src/cw.ts`, `ui/src/ctl/cw.tsx`, §2.1), item 2
(FSK/RTTY; `crates/fsk`, `crates/fsk-wasm`, `ui/src/rtty.ts`,
`ui/src/ctl/rtty.tsx`, §2.2), items 3 and 4 (SSTV and HF fax;
`crates/img`, `crates/img-wasm`, `ui/src/img.ts`, `ui/src/ctl/img.tsx`,
§2.3-2.4); item 5 (SITOR-B decodes; NAVTEX messages listed per signal; not
tried on air), item 7 (the neural CW decoder, §5) and item 9 (wide
skimming, §6).

## 1. Todo

| # | decoder | source to start from | done when |
|---|---|---|---|
| 1 | **CW** | datarx `wahrx/cw.py` (+ `cwskim.py` for several signals in the passband) | text and WPM for the signal at the CW offset; others in the passband listed too |
| 2 | **FSK / RTTY**, with parameter detection | datarx `wahrx/fsk.py` | shift, baud, framing (data bits, parity, stop bits), polarity and charset found from the signal; ham RTTY and DWD decode with no settings |
| 3 | **SSTV** | new (nothing in datarx) | VIS-detected Martin, Scottie, Robot and PD modes as images, slant corrected |
| 4 | **HF fax** | new (nothing in datarx) | DWD/NOAA-style charts as images: start/stop tones, phasing, IOC 576/288, 60/90/120 LPM, slant corrected |
| 5 | NAVTEX / SITOR-B | datarx `wahrx/fsk.py` (already decodes it) | falls out of item 2; a NAVTEX view (B1-B4 header, message list) |
| 6 | later: DSC, time codes (DCF77, MSF, WWVB...) | new | as in `feature-gap.md` §2.4 |
| 7 | optional: **neural CW** | trained on datarx-style synthetic CW (§5) | beats item 1 on the synthetic sets and on real recordings; shipped only then |
| 8 | optional: **signal identifier** | trained on synthetic + labelled recordings (§5) | names the mode of a signal in the passband (RTTY 50/450, fax, SSTV, FT8, STANAG 4285...) and starts its decoder |
| 9 | **wide skimming**: a channelizer bin (60 kS/s IQ, ~40 kHz) to the browser for the CW and RTTY skimmers | — | a whole CW or RTTY sub-band skimmed at once in the page; the unit only passes the bin on |

Order: 1 and 2 first (code exists to port, most asked), then 4 and 3
(images, a new panel type), then the rest.

## 2. How (same pattern as JTTY)

* A Rust crate per decoder in `crates/` (DSP only, no I/O; streaming
  `push(&[f32])` API like `jtty::Receiver`), a `*-wasm` crate, a worker in
  `ui/src/<name>/worker.ts` fed from the page's audio resampled to 12 kHz
  (`jtty.ts`'s resampler).
* datarx (the author's own SDR toolkit, Python package `wahrx`) is ported to Rust, not
  shipped as Python. Its tests (`tests/test_cw.py`, `test_fsk.py`,
  `tests/synth.py`) become the crates' tests: synthetic signals at given
  SNR, fading and keying jitter, with datarx's character error rates as
  the bar (README "CW decoder" and "FSK / RTTY" sections). datarx is MIT
  (its author's choice, 2026-11-15), so the ports are GPL-3.0-or-later
  like the rest of web888d; each crate header names datarx's module as
  its origin, as `jtty` does for jtty-cuf. Same author, so no separate
  MIT notice is needed.
* datarx works on complex baseband from a wideband capture; here the
  input is real audio from one passband. Take the CW and FSK cores (tone
  estimation, demodulation, framing, charset), not the skimmers' FFT
  banks; the passband (≤ ~3 kHz) is analysed whole for several signals.

### 2.1 CW (from `cw.py`)

* Carrier: strongest keyed line in the passband (default: the CW mode's
  pitch offset); speed hypotheses 9-45 WPM, matched filter per speed,
  adaptive dot length, learned character/word gap; threshold from the
  tracked noise floor (nothing decoded without a keyed signal).
* Several signals in the passband: one decoder per keyed line (QSO both
  sides 20-25 Hz apart decode in datarx).
* UI: text per signal, WPM and SNR; click a line to pick it.
* **Done (2026-11-15).** The skimmer on 12 kHz audio: a 512-point
  spectrogram of the passband (21 ms hop, so 40 WPM key-up gaps show)
  finds keyed lines; each gets a 500 S/s complex channel (289-tap FIR,
  primed from the last 8 s of audio), re-decoded over its last 30 s every
  2 s; characters are emitted once 3 s (40 dots) old. Ported tests pass
  (speeds, prosigns, jitter, noise only, a two-operator QSO, two signals
  in SSB audio, a steady carrier ignored). Character error rates
  (`cargo run --release -p web888-cw --example cer`, 20 WPM, 20
  messages each; SNR in 2500 Hz):

  | SNR | clean | 10 % jitter | QSB 0.2 Hz |
  |---|---|---|---|
  | +6 dB | 0.0 % | 2.2 % | 9.8 % |
  | 0 dB | 0.0 % | 2.4 % | 26.2 % |
  | −3 dB | 0.4 % | 2.2 % | 38.1 % |
  | −6 dB | 10.2 % | 16.2 % | 57.2 % |
  | −8 dB | 57.8 % | 61.7 % | 67.2 % |

  Four signals in 60 s of audio: 4 % of real time natively. In the page
  (`ui/tools/cdp/cw.mjs`) on the 40 m recording: a contest exchange at
  7018.9 kHz (20 WPM, USB) and a weak CQ at 7022.3 kHz (26 WPM, CW mode)
  decode as datarx reads them. One difference from datarx: the levels
  ignore the filters' start-up at the window's ends (datarx decoded a
  spurious E there). These numbers are item 7's baseline.

### 2.2 FSK / RTTY (from `fsk.py`)

Everything detected from the signal, with manual overrides:
* tones (pair whose energies alternate; refined from phase), so shift and
  centre are known; sideband (USB/LSB) folded into the polarity;
* baud: shortest transition interval, refined and snapped to standard
  rates (45.45, 50, 75, 100, 110, 150, 200, 300...);
* async framing: 5-9 data bits, parity (none/even/odd), stop bits
  (1/1.5/2), polarity, chosen by fewest framing errors;
* charset: ITA2 / US-TTY figures, ASCII 7/8 bit LSB/MSB first; unshift on
  space decided from the text;
* synchronous: reversals identified; SITOR-B / NAVTEX decoded; ARQ-E,
  SITOR-A identified only.
* UI: the detected parameters shown (and lockable), then streaming text;
  re-detection when framing errors pile up.
* **Done (2026-11-15)**, without the manual overrides (not needed so
  far). The skimmer on 12 kHz audio: a 768-point spectrogram (15.6 Hz
  lines) of the passband, tone pairs every 2 s from the last 8 s; each
  gets a 4 kS/s channel (primed from the last 8 s), analysed once 4 s are
  in, locked when believable, then decoded as a stream. Identified but
  undecodable signals (reversals, 4-of-7 codes) are listed as such. The
  panel shows each signal's rate, shift, framing, polarity (in RF terms:
  LSB mirrors) and character set, its SNR and framing errors, and the
  text; a list of stations (DWD, NAVTEX, ham RTTY) tunes USB with the
  centre at 1 kHz. All of datarx's FSK tests pass (five framings with
  polarity, idle gaps, −5 dB, selective fading, SITOR-B, reversals,
  noise, USOS from the text, streaming in blocks), plus two signals and
  reversals in SSB audio. Ham RTTY (`cargo run --release -p web888-fsk
  --example cer`, five runs each): every parameter found at 0 to −7 dB
  (4 of 5 at −5 dB), none at −9 dB; character errors with known
  parameters 0 % at −3 dB, 0.1 % at −5, 3.5 % at −7, 25 % at −9 (datarx:
  0, ~1, ~5, ~28 %). One fix over datarx: before the first space tone
  (an idle mark), the detector divided noise by an unknown level and
  decoded garbage; the heard tone's levels now stand in. In the page on
  the 40 m/30 m recording: DWD 10100.8 kHz locks within 15 s as 50 Bd,
  450 Hz, 5N1.5, mark lower, ITA2 (as datarx), with the Baltic sea
  forecast as text.

### 2.3 SSTV (new)

* VIS code (1900 Hz leader, 1200 Hz break) picks the mode; mode table for
  Martin 1/2, Scottie 1/2/DX, Robot 36/72, PD 90/120/180 at least.
* FM demodulation of 1500-2300 Hz luminance, sync at 1200 Hz; line timing
  from the sync pulses, slant from a fit over lines (the sound card clock
  and the receiver's rate differ a little).
* Image drawn as it arrives; PNG save. A mode can be forced without VIS.
* **Done (2026-11-15)**, without forcing a mode (VIS only so far). Modes:
  Martin 1/2, Scottie 1/2/DX, Robot 36/72, PD 50/90/120/160/180/240. The
  FM demodulator (shared with fax) mixes 1900 Hz to complex, low-passes
  ±1200 Hz and gives the instantaneous frequency at 6 kS/s. VIS: leader,
  start bit, 7 bits + even parity, stop bit, each 30 ms bit's middle 20
  ms. Syncs: 1 ms smoothed frequency under 1350 Hz for half a sync,
  fitted as start + k × period with outliers out (so a clock off by a
  few hundred ppm doesn't slant the picture; slant beyond ±1 % is
  refused); the picture is redrawn when 8 and 32 syncs are in. A pixel
  is the mean frequency over its time. Tests (an encoder in the crate):
  every mode round-trips with mean error under 12/255; Martin 1,
  Scottie 1, Robot 36 and PD 120 with the sender's clock 400 ppm off and
  ~15 dB SNR under 30/255, the slant fitted to within 3 ppm. Not tried
  on air yet.

### 2.4 HF fax (new)

* FM demodulation, 1500 Hz black / 2300 Hz white (dial = carrier
  − 1.9 kHz on USB).
* APT start tones (300 Hz for IOC 576, 675 Hz for IOC 288) and stop tone
  (450 Hz); phasing lines set the line start; LPM 60/90/120 (default 120).
* Slant from the phasing signal and adjustable by hand; image drawn line
  by line; PNG save. Starts without the start tone too (manual).
* **Done (2026-11-15).** APT tones from the share of the luminance's
  variance at 300/675/450 Hz in 0.25 s windows (one second of them);
  phasing: 40 lines from 1 s after the start tone, the 5 % pulse found
  whichever way it is (dark on white or the reverse) to a fraction of a
  bin, its drift fitted (outlier lines dropped) for the line period and
  start; the chart is redrawn then. Slant (±50 ppm) and shift (±2 %)
  nudges redraw from the kept luminance. Tests: a DWD-style fax (start
  tone, 30 s phasing, chart, stop tone) at 0, +300 and −250 ppm with
  both phasing polarities: the slant fitted within 60 ppm and the chart
  within 0.15 mean error; a fax started by hand. In the page on the 40 m
  recording: DWD 7880 kHz, started by hand (the 60 s recording has no
  start tone).

## 3. Shared UI

* A decoder panel the text decoders share (CW, RTTY, NAVTEX, JTTY), and
  an image panel (SSTV, FAX) with a save button.
* The audio-passband waterfall (`feature-gap.md` §2.4, "secondary
  waterfall"): shows the tones and lets a click set the decoder's
  frequency; useful for all of the above.
* Images and logs stay in the browser (download); the server-side files
  gallery is a separate item.

## 4. Test material

* Synthetic: ported from datarx's tests; SSTV and FAX encoders in the
  test code (a known image round-trips).
* On air: the HackRF dev source (`--backend iq`) and recordings under
  `/data/iq`: 20 m CW, DWD RTTY (10100.8 / 11039 / 14467.3 kHz), DWD fax
  (3855 / 7880 / 13882.5 kHz), NAVTEX 518 kHz, SSTV on 14.230 MHz.

## 5. Neural decoders (optional, after the classical ones)

* **Neural CW**: log spectrogram of the passband around the signal →
  small CNN + bidirectional GRU/LSTM → CTC over the Morse alphabet; beam
  search with a small language model (callsigns, Q codes, QSO phrases).
  Trained on synthetic CW from the classical crate's test generator
  (SNR, QSB, keying jitter, speed 10-45 WPM, QRM, QRN), so labels are
  free. The classical decoder (item 1) is the baseline and the fallback.
* **Signal identifier**: a small CNN on a few seconds of spectrogram;
  classes from the decoders we have plus common utility modes.
* Not neural: RTTY (the demodulator is near optimal; a language model
  for lost LTRS/FIGS shifts helps more), SSTV/FAX (an enhancing network
  would invent detail in charts), FT8/FT4/WSPR/JTTY (strong FEC already).
* Training: PyTorch on the workstation's GPU (`tools/ml/`, not shipped),
  export to ONNX. Inference in the browser: `tract` in the wasm crate (one
  test path through `cargo test`), weights fetched lazily (< 1 MB).

### 5.1 Neural CW: done (2026-11-15)

`tools/ml/cw/` (training, `README.md`), `crates/cw/src/net.rs` (inference
without a framework, weights embedded as f16: 96k parameters, 190 kB),
the CW panel's Neural button (on by default). Features: a 16-point STFT
every 4 ms, log power of the 5 bins within ±62.5 Hz of the carrier (so
nearby interference is seen), normalised by the noise floor; model: three
1-D convolutions (batch norm folded in at export), 2-layer bidirectional
GRU (48), CTC over 58 characters. Training: 30k steps of 128 synthetic
12 s windows (10-45 WPM, jitter, dash ratio, Farnsworth, −12..+20 dB,
fading, a second CW signal, impulse noise, 5 % noise only; text from QSO
phrases, callsigns, words and random groups), 30 min on an RTX 5070. The
Rust inference matches PyTorch to 2e-3 (`net::tests`).

Against the classical decoder, on the Rust synthesiser (not the one it
trained on), character errors (`--example cer`):

| SNR | clean | 10 % jitter | QSB 0.2 Hz |
|---|---|---|---|
| +6 dB | 0.0 / 0.0 % | 2.2 / 0.0 % | 9.8 / 1.5 % |
| 0 dB | 0.0 / 0.0 % | 2.4 / 0.0 % | 26.2 / 6.8 % |
| −3 dB | 0.4 / 0.0 % | 2.2 / 0.0 % | 38.1 / 11.8 % |
| −6 dB | 10.2 / 0.0 % | 16.2 / 0.2 % | 57.2 / 21.5 % |
| −8 dB | 57.8 / 0.4 % | 61.7 / 1.5 % | 67.2 / 29.5 % |
| −10 dB | 80.8 / 2.9 % | 80.3 / 5.9 % | 78.4 / 39.8 % |

12-45 WPM at 0 dB with jitter: 1.4-5.8 % / 0.0 %. A 12 s window takes
72 ms natively. In the skimmer, the classical decoder still gives speed,
SNR and frequency; the network's text (last 12 s every 2 s) replaces its
text, and four characters of its own make a signal count as decoding (a
−7 dB hand-keyed station decodes, noise stays quiet: `tests/cw.rs`). On
the 40 m recording it reads more than the classical decoder (both sides
of a QSO, "5NN 73 TU" where the classical read "7E M TU"). Word spaces
(2026-11-15): on air they went missing between transmissions ("SN1TTEST",
"TESTOM8DD": a contest station's CQ, a pause, the CQ again), as training
had only word gaps of ~7 dots. Fixed twice: training pauses of 0.5-4 s
between phrases (70 %) and now and then mid-line, labelled as a space (a
10k-step fine-tune); and in the skimmer a silence over 2 s or 25 dots
between emitted characters is a space whatever the decoder says. On a 90 s
live capture of 7000-7040 kHz in a contest (`--example replay`), words of
10+ characters (run together): 7.9 % → 2.6 % (the rule) → 1.2 % (both);
what's left is pile-ups. Error rates as before or slightly better
(−8 dB 0.2 %, −10 dB 2.3 %).

## 6. Wide skimming (item 9): done (2026-11-15)

The audio is one 12 kS/s channel (±5 kHz at most); the channelizer's
bins are 60 kS/s complex, 30 kHz apart and clean within ±21 kHz of their
centre, so one bin is a whole CW sub-band or RTTY segment.

* **Server** (`crates/web/src/wideiq.rs`): the client asks with
  `{"type": "wide", "on": true, "hz": dial}`; the session subscribes to the
  bin tap (the one zoomed waterfalls use) for the bin nearest `hz` and
  sends its samples as they are read: frames of kind 3, 2048 complex
  samples as i8 pairs scaled by the block's peak (header: frame number,
  bin centre, rate, scale; ~1 Mbit/s). At most 4 streams at a time (each
  is a bin read, ~1 % of an A9 core, and the link). The hello's `wide`
  says whether the server has bins (bins mode, or the channelizer
  bitstream's tap). Frame fixtures are shared with the UI's tests.
* **Skimmers**: `web888_cw::Skimmer::new_iq` and
  `web888_fsk::Skimmer::new_iq` take complex IQ at any rate (a multiple of
  their channel rate) and a band of offsets (±20 kHz); real audio goes the
  same way as complex with a zero imaginary part. Spectrogram lines stay
  23.4 Hz (CW) and 15.6 Hz (FSK) at any rate, channel filters keep their
  transition band (more taps), mixers are complex rotators. The CW
  skimmer takes up to 60 signals in IQ, the FSK one 24.
* **UI**: a Wide button in the CW and RTTY panels (when the server has
  bins); the stream follows the dial (asked again when the dial leaves the
  bin's middle 30 kHz); the panel says the range skimmed; RF = bin centre
  + offset.
* **Cost** (24 CW stations, half of them weak, in 40 s of a bin;
  `cargo run --release -p web888-cw --example wide`): natively classical
  26 %, neural 29 % of a core, all 24 read (classical 23). In WebAssembly
  (V8, `simd128`): 36 % and 46 % of a core, in the worker. Getting there:
  the network's dot products with 8 accumulators (vectorised; a 12 s
  window 72 → 21 ms), its window 8 s, and with the network on the
  classical decoder runs every third time per signal (it only gives speed,
  SNR and frequency then).
* **Tests**: six CW stations over ±19 kHz of a bin at 14-34 WPM all
  decode; ham RTTY and 100 Bd ASCII 19 kHz apart both lock and decode. In
  the page (iq backend in bins mode, the 40 m recording): with the dial at
  7020 kHz it skims 6990-7030 kHz, four CW signals at once (both sides of
  a QSO at 7018.9, a weak CQ at 7022.3, a CQ at 7008.9); RTTY on the bin
  around 10100.8 kHz locks DWD as in audio mode.
