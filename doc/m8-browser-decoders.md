# M8: more decoders in the browser (plan B)

Status: in progress (planned 2026-11-15). Item 1 (CW) done: `crates/cw`,
`crates/cw-wasm`, `ui/src/cw.ts`, `ui/src/ctl/cw.tsx` (§2.1). This is
`feature-gap.md` §4 item B: more decoders that run on the page's own
audio, as JTTY and FT8/FT4/WSPR do. Nothing here runs on the unit, so the
A9's CPU limits don't apply.

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

### 2.3 SSTV (new)

* VIS code (1900 Hz leader, 1200 Hz break) picks the mode; mode table for
  Martin 1/2, Scottie 1/2/DX, Robot 36/72, PD 90/120/180 at least.
* FM demodulation of 1500-2300 Hz luminance, sync at 1200 Hz; line timing
  from the sync pulses, slant from a fit over lines (the sound card clock
  and the receiver's rate differ a little).
* Image drawn as it arrives; PNG save. A mode can be forced without VIS.

### 2.4 HF fax (new)

* FM demodulation, 1500 Hz black / 2300 Hz white (dial = carrier
  − 1.9 kHz on USB).
* APT start tones (300 Hz for IOC 576, 675 Hz for IOC 288) and stop tone
  (450 Hz); phasing lines set the line start; LPM 60/90/120 (default 120).
* Slant from the phasing signal and adjustable by hand; image drawn line
  by line; PNG save. Starts without the start tone too (manual).

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
