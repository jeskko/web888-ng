# M8: more decoders in the browser (plan B)

Status: in progress (planned 2026-11-15). Done: item 1 (CW; `crates/cw`,
`crates/cw-wasm`, `ui/src/cw.ts`, `ui/src/ctl/cw.tsx`, §2.1), item 2
(FSK/RTTY; `crates/fsk`, `crates/fsk-wasm`, `ui/src/rtty.ts`,
`ui/src/ctl/rtty.tsx`, §2.2), items 3 and 4 (SSTV and HF fax;
`crates/img`, `crates/img-wasm`, `ui/src/img.ts`, `ui/src/ctl/img.tsx`,
§2.3-2.4); item 5 (SITOR-B decodes; NAVTEX messages listed per signal; not
tried on air), item 7 (the neural CW decoder, §5), item 9 (wide
skimming, §6), item 10 (PSK31/63/125; `crates/psk`, `crates/psk-wasm`,
`ui/src/psk.ts`, `ui/src/ctl/psk.tsx`, §2.5) and item 6 (DSC in the RTTY
panel, `crates/fsk/src/dsc.rs`, §2.8; time codes, `crates/timecode`,
`crates/timecode-wasm`, `ui/src/timecode.ts`, `ui/src/ctl/timecode.tsx`,
§2.9).

## 1. Todo

| # | decoder | source to start from | done when |
|---|---|---|---|
| 1 | **CW** | datarx `wahrx/cw.py` (+ `cwskim.py` for several signals in the passband) | text and WPM for the signal at the CW offset; others in the passband listed too |
| 2 | **FSK / RTTY**, with parameter detection | datarx `wahrx/fsk.py` | shift, baud, framing (data bits, parity, stop bits), polarity and charset found from the signal; ham RTTY and DWD decode with no settings |
| 3 | **SSTV** | new (nothing in datarx) | VIS-detected Martin, Scottie, Robot and PD modes as images, slant corrected |
| 4 | **HF fax** | new (nothing in datarx) | DWD/NOAA-style charts as images: start/stop tones, phasing, IOC 576/288, 60/90/120 LPM, slant corrected |
| 5 | NAVTEX / SITOR-B | datarx `wahrx/fsk.py` (already decodes it) | falls out of item 2; a NAVTEX view (B1-B4 header, message list) |
| 6 | DSC, time codes (DCF77, MSF, WWVB, JJY, WWV/WWVH) (done, §2.8-2.9) | new (ITU-R M.493; PTB, NPL, NIST SP 432, NICT) | calls read with their ECC; the time and date read with the stations' checks |
| 7 | optional: **neural CW** | trained on datarx-style synthetic CW (§5) | beats item 1 on the synthetic sets and on real recordings; shipped only then |
| 8 | optional: **signal identifier** (done, §5.3) | trained on synthetic + labelled recordings (§5) | names the mode of a signal in the passband (RTTY 50/450, fax, SSTV, FT8, STANAG 4285...) and starts its decoder |
| 9 | **wide skimming**: a channelizer bin (60 kS/s IQ, ~40 kHz) to the browser for the CW and RTTY skimmers | — | a whole CW or RTTY sub-band skimmed at once in the page; the unit only passes the bin on |
| 10 | **PSK31/63/125** (BPSK, Varicode) | new (G3PLX's Varicode) | every BPSK signal in the passband found (carrier, rate) and decoded |
| 11 | **HFDL** (aircraft data link, 1800 Bd PSK; ARINC 635, dumphfdl GPL-3 as the reference) | new | squitters, logons, position reports and ACARS decoded, checked by their CRCs; aircraft positions and ground stations on the map (§2.6) |
| 12 | **JS8** (JS8Call: FT8's waveform, its own code and frames; Normal, Fast, Turbo, Slow) | the FT8 decoder, generalised | heartbeats, directed messages and free text, frames joined per frequency; senders with a grid on the map (§2.7) |
| 13 | later: ALE 2G, Olivia | new | — |
| 14 | planned: **neural noise reduction**, one model each for SSB voice, CW and AM broadcast (§5.4) | the papers in §5.4; trained like the neural CW | SSB voice and AM speech clearer than the spectral subtraction (`crates/dsp/src/nr.rs`) by Whisper's word errors on real HF recordings, CW elements easier to copy by ear, AM music not damaged, nothing invented at low SNR |
| 15 | **label every signal** (done, first version, §5.5) | the identifier and `measure.rs` | every signal in the waterfall's view marked with its class, kept up to date as the view moves |

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
* **Slow narrow FSK (2026-11-28)**: RDL 18.1 kHz (50 Bd, 75 Hz shift,
  sync) was found and analysed right (4-of-7 code, words spread evenly
  over all 35: enciphered, nothing to read) but never listed: the ident
  gate wanted SNR ≥ −3 dB in 2500 Hz, and a clean 50-Bd signal at 11 dB
  per bit reads −5.7 dB. The gate now also passes ≥ 10 dB per bit (in
  the baud's bandwidth). Same idents as before on three 10-min 5 MHz
  recordings.
* **A contest as a test set (2026-11-15)**: 10 minutes of the WAE DX
  RTTY contest recorded off air on 20 m and 40 m (HackRF, 4 MS/s), RTTY
  segments cut to 40 kS/s IQ and skimmed (`--example replay`; 600 s in
  20-40 s, 16-38 stations locked per 34 kHz). No transcript exists, so
  errors are found by spellcheck: a word one edit from a word the
  recording repeats (a running station's call, TEST, a serial). Each
  suspect was decoded again with receiver variants (`--example dive`,
  soft bits per character). Of 238 suspects on three slices about 1 in 9
  was sent as read (clean bits in every variant: the operator's typo, a
  call heard once); a third were weak in every variant (selective fades
  faster than the 0.6 s threshold tracking, a static crash in a weak
  signal); the rest were readable by some variant. Compared like for like (`--example spans`: every
  skimmed span decoded from the same channel with each filter), a Hann
  tone filter 1.4 bits long in place of the 1-bit boxcar reads 8 % more
  of the recordings' words right (5118 against 4720) for the same
  errors (326 against 324): it keeps the boxcar's noise bandwidth with
  sidelobes 31 dB down rather than 13, so neighbours a few hundred Hz
  away leak far less. On synthetic RTTY it helps a little too (−7 dB:
  2.6 % character errors against 3.5 %). Faster threshold tracking
  for the fast fades only loses (0.2 s: 4958 right), as noisier levels
  cost every signal; instead each tone's half of the decision is
  weighed by its strength now (a peak follower releasing in 0.4 s,
  against its tracked levels), so while one tone is faded the other
  decides alone against its own midpoint: 5269 right, 304 errors (with
  both tones up the decision is unchanged; on synthetic white noise it
  costs a little, −7 dB 2.9 %). Done for async; SITOR-B keeps the boxcar
  its bit alignment is found with.
* **Weak characters shown, not guessed**: a character whose bits came
  near the threshold (smallest |soft decision| under 0.3, or a framing
  error) is reported weak (`Event::Text` `weak`, `"weak"` in the worker
  JSON) and dimmed in the panel. On the contest slices the wrong
  character is marked in 53-67 % of the substitution errors (the rest
  mostly sent as read, or flipped cleanly by QRM) while 1.3-1.8 % of the
  characters of words read right are; keyup junk between overs is
  dimmed too. Not done on purpose: correcting a weak character from the
  words a station repeats (a guess, wrong for a call heard once or the
  operator's own typo).
* **Overs with pauses (2026-11-16)**: a contest station on 7092 kHz,
  strong (~20 dB over the noise in 240 Hz) and sending 5-12 s overs
  with pauses, was hardly decoded on the unit. The skimmer looks every
  2 s: it found him 1.5 s into an over, analysed the last 8 s (mostly
  pause), got nothing, dropped him and ignored the frequency for 2
  minutes, again and again. Now when the whole buffer's analysis fails
  and the signal pauses, the longest stretch it is on (50 ms blocks 6 dB
  over the quietest fifth, pauses up to 0.5 s bridged) is analysed, once
  3 s of it are heard; a pair never heard that long is forgotten, not
  ignored. On a 90 s recording of him: locked, his CQ read in every
  over (none before). The contest slices gain too (words in the
  vocabulary 2567 → 2632, 1131 → 1205, 1732 → 1839 for 46/20/43 locks
  against 40/15/40; one-edit-off ratio 8.4 → 8.7, 6.6 → 6.3, 7.6 →
  7.5 %). Test: `skimmer_overs_with_pauses` (synthetic, the first look
  at 1 s of the first over; none decoded before).

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

### 2.5 PSK31/63/125 (new): done (2026-11-15)

* **Receiver** (`crates/psk`): a channel at 16 samples a symbol; the
  matched filter is PSK31's pulse (a Hann window two symbols long);
  symbol timing from its output's energy; differential detection (no
  phase change a 1, a reversal a 0); a frequency loop on p² (the phase
  change doubled); a squelch on the mean |cos| of the phase changes;
  Varicode (characters end at "00"; runs longer than any code are a
  steady tone, not text). A character whose smallest |cos| is under 0.5
  is weak (dimmed, as in RTTY; nothing is replaced).
* **Skimmer**: every 2 s, peaks of the last 4 s's spectrum (2.9 Hz
  lines). A peak is a candidate when its squared signal has one line
  (twice the carrier, which it gives to a fraction of a hertz: FSK
  squares to a line per tone) and its envelope a line at 31.25, 62.5 or
  125 Hz (the rate). It is decoded from the last 8 s and kept when two
  blocks of 32 symbols there read as clean BPSK: p² coherent, mean |cos|
  over 0.85, at most 10 % of changes near 90° and at least 20 % clean
  reversals. On the 40 m contest recording that last test took the
  false locks on RTTY and CW from 59 to 0 (real PSK31 blocks: |cos| ~1,
  30-50 % clean reversals; RTTY and CW taken for BPSK: 0.7-0.9, at most
  16 %).
* **Synthetic** (`--example cer`, five runs, SNR in 2500 Hz, character
  errors of the text read): PSK31 found at 0 to −10 dB (4 of 5 at −10),
  0.8 % / 0.9 % / 2.2 % at 0 / −5 / −8 dB, 27 % at −10; PSK63 down to
  −8 dB (12 %); PSK125 down to −5 dB (11 %). Locks in 3 s. The
  demodulator alone, with the carrier known, reads PSK31 at −10 dB almost
  cleanly: the skimmer loses a dB or two, partly the benchmark's SNR.
  0.13 % of a core per second of audio.
* **On air** (`--example replay` on the 2026-11-15 recordings): 20 m
  14070-14074 kHz, four PSK31 stations found and read (a special event
  station's CQs and QSOs, an Italian ragchew, a /P CQ), nothing false in
  the FT8 segment next to them; 40 m 7023-7057 kHz in the RTTY contest,
  the one PSK31 QSO there and nothing else.
* **Page**: PSK in the Decoders list; the panel lists each signal (RF,
  PSK31/63/125, how cleanly it reads) and its text, with the usual
  frequencies to tune to; the identifier's PSK31 rows open it. Checked in
  a browser: the worker with `psk.wasm` on synthetic audio locks PSK31 at
  1000.0 Hz and PSK63 at 1700.0 Hz and reads both texts without an error;
  on the unit at 7040 kHz at night nothing was on. Not yet: wide
  skimming (the crate takes IQ already), QPSK.

### 2.6 HFDL (new): done (2026-11-15)

HFDL is the HF data link of airliners and 16 ground stations: 1800 Bd
PSK bursts (BPSK, QPSK or 8PSK; 300 to 1800 bps) in 2.4 s TDMA slots,
1440 Hz above the listed (USB dial) frequency. ARINC 635 isn't public;
dumphfdl (GPL-3.0, like this project) was read as the reference for the
burst format, the PDUs and the ground station table, and libacars (MIT)
for ACARS framing. Nothing was built or run from either. Two of its
constants differ from a plain reading: on air they were found from the
signal itself (`--example diag`).

* **Burst** (`crates/hfdl`, `phy`): a 448-symbol prekey, the 127-symbol A
  sequence twice, M1 (127 symbols: one of eight rotations of an
  m-sequence, the rate and single or double slot), M2 (15), nine
  15-symbol T sequences, then 72 or 168 frames of 30 data symbols and a
  15-symbol T probe. Data: a 120-bit scrambler (180° turns), a 40-row
  block interleaver, a K=7 rate 1/2 code (300 bps sends each coded bit
  twice), Gray-coded M-PSK. The scrambler's LFSR starts from 0x4d4b: the
  bits common to many bursts (zero padding) repeat with it every 120
  symbols. The code's two outputs go out as 0x6d then 0x4f: a strong
  burst re-encodes exactly that way, and its 300 bps pairs agree 100 %
  once descrambled and deinterleaved.
* **Receiver** (`front`, `demod`): audio (or IQ) to 4 samples a symbol
  through a root raised cosine filter. Detection: the A pair correlated
  in 16 pieces whose powers add (a frequency offset turns the pieces;
  ±40 Hz is fine), the best within 130 symbols of the first over
  0.18 (noise ~0.06). Frequency from the pieces' phases. A 17-tap
  half-symbol equaliser fitted by least squares to the A pair, then
  normalised LMS on the known symbols only, with a phase loop. M1 picks
  the burst type; under 0.4 the burst is dropped. Soft bits use the gain
  interpolated between the probes on either side of each 30-symbol
  block: on the polar path from Barrow the channel turns 30-60° a frame
  (auroral fading), which a forward-only tracker followed too late.
* **Messages** (`pdu`, `skim`): squitters (the ground station, its TDMA
  frame, the frequencies it and two others use), MPDUs and their LPDUs
  (logon request, confirm, logoff, denied), HFNPDUs (position reports,
  frequency data, system table parts) and ACARS (registration, flight,
  label, text). Each part's own CRC must pass; a part that fails is
  shown as such, never mended. An aircraft's ICAO address comes from the
  logon confirm that gave its ID; its flight and registration from its
  messages. A different flight or registration on the ID, or 30 minutes
  unheard, forgets them (IDs are reused, and a logoff can be missed).
* **Recordings** (2026-11-15 16:20 UTC 40 m, 6646 kHz, Barrow and its
  aircraft; 2026-10-23 8942 kHz, Shannon; `/data/iq/hfdl`): 65 of 89
  bursts decoded (40 + 25), from 33 at the first version; every burst
  with a probe SNR of 2.4 dB or more. What's left is mostly Barrow's own
  squitters at about −3 dB in fast fading. USB audio made from the
  recordings decodes as the IQ does (64). Synthetic: all eight burst
  types at 30 dB; 600 and 1800 bps with a 23 Hz offset, a second path
  1.2 ms late at 0.6 and 12 dB SNR. 0.4 % of a core.
* **On air** (the unit, 2026-11-15 21:35 UTC, Reykjavik 5720 kHz through
  the page): 54 of 66 bursts in four minutes: squitters, logons,
  position reports, ACARS. At that hour 6646, 8942 and the other
  channels tried were silent.
* **Page**: HFDL in the Decoders list. The panel lists decoded bursts,
  newest first (UTC, ground station, direction, rate, SNR), and their
  parts, with the channels to tune to (USB). The worker with `hfdl.wasm`
  decodes the same bursts as the native build (24 of 33 in a 300 s
  test). The map (Browser layer, band "HFDL") shows the aircraft at
  their reported positions, dated by the report's own time (often an
  hour older than the burst), and the ground stations heard sending;
  on the unit, 5720 kHz at 23:00 UTC: Reykjavik and seven aircraft in
  six minutes. Not yet: the over-the-air system table,
  more than one channel at a time (the wide IQ path), ACARS reassembly
  of multi-block messages.

### 2.7 JS8 (new): done (2026-11-16)

JS8Call (GPL-3.0) was read as the reference; nothing of it was built or
run.

* **Frames** (`crates/ftx`, `js8`): FT8's 79 symbols of 8-FSK at FT8's
  timing for Normal, but 87 bits (72 text bits, 3 flag bits, a CRC-12:
  boost's augmented CRC, polynomial 0xc06, XOR 42), a (174,87) LDPC code
  whose codeword is the parity then the message, tones straight from the
  3-bit words (no Gray code), its own Costas arrays (one for Normal,
  three others for the rest), plain FSK. The FT8 decoder now takes a
  Costas set, tone map, code and start time (`Config::js8`, `t0`; the
  belief propagation decoder takes any code's checks): FT8 and FT4
  decode exactly as before on both corpora (FT8 1623/1842, 1458/1600,
  1173/1267; FT4 468/488). No OSD for JS8: with a 12-bit CRC it would
  find false frames (JS8Call doesn't use it either).
* **Submodes** (`crates/js8`): Fast (10 Bd, 10 s), Turbo (20 Bd, 6 s)
  and Slow (3.125 Bd, 30 s) are Normal's frame at another symbol length,
  so each period is resampled in the frequency domain to Normal's timing
  (×1.6, ×3.2 in time; Slow ×0.5, its band in two halves) and decoded by
  the same decoder; frequency, time and SNR mapped back.
* **Messages** (`message`, `jsc`): the flag bits say first frame, last
  frame, fast data; the 72 bits start with the frame type: heartbeat
  (callsign, grid, @HB or @ALLCALL CQ), compound callsign, directed
  (from, to, one of 32 commands, a number or SNR), data. Free text is
  JS8Call's dictionary compression (262144 entries, `data/jsc_words.txt`
  from its `jsc_map.cpp`, embedded in the wasm: 2.7 MB, 1.3 MB as served
  gzipped), or the old Huffman code. Text as JS8Call shows it.
* **Synthetic** (`--example snr`, 20 frames a point, SNR in 2500 Hz):
  50 % at −20 dB Normal, −19 Fast, about −16 Turbo, −23.5 Slow (each as
  its symbol rate predicts); no false frame from noise in 20 periods
  each.
* **Recording** (2026-11-15 16:21 UTC, 7078 kHz, `/data/iq/js8`): 47
  frames in 10 minutes (44 Normal, 2 Fast, 1 Slow), every one readable:
  heartbeats and their SNR replies, CQs, a QSO's free text across
  frames. 20 m had 4 then, 80 and 15 m none. All four submodes cost 4.5 s
  for the 10 minutes natively, 11 s in the browser (~2 % of a core).
* **Page**: JS8 in the Decoders list. Four slot collectors (one per
  submode) on the audio; each period is decoded as it ends. The panel
  joins frames per frequency into messages (first and last frame flags,
  as JS8Call's band activity), lists the stations heard, shows frames
  and periods decoded, and has the usual dials. On the unit, 7078 kHz at
  22:30 UTC: heartbeat replies within five minutes; 3578 and 10130 kHz
  were quiet. The map (Browser layer) places senders by a heartbeat's
  or compound's grid, a "GRID" sent as text, or a locator learnt from
  any mode (FT8 included); a click tunes to the usual dial. On a replay
  of the 40 m recording (resampled to 1.92 MS/s for `--backend iq`):
  the three stations that sent grids. Not yet: the wide IQ path.

### 2.8 DSC (new): done (2026-11-16)

Digital selective calling (ITU-R M.493, written from the Recommendation)
on MF/HF: 100 Bd FSK, 170 Hz shift, in the RTTY panel with the other
FSK signals (`crates/fsk/src/dsc.rs`). The FSK analysis tries it after
SITOR-B: a 100 Bd signal whose ten-bit words pass their check bits (seven
information bits, LSB first, and the count of their zeros) or that holds
a call is decoded as DSC.

* **A call**: the phasing (DX symbol 125, RX 111 down to 104) is looked
  for at every bit, accepted on §3.3's rule (two DX and one RX, two RX and
  one DX, or three RX). Each character is its DX copy if that passes its
  check, else its RX copy (five slots later); where both fail or disagree,
  the choice that makes the error-check character right is taken (one
  lost character, or up to four disagreements).
* **Shown**: one line a call: the format (distress alert, all ships,
  individual, group, geographic area, semi-automatic), category, to whom
  and from whom (MMSI and what kind: coast, ship, group, SAR aircraft...),
  the telecommands or the distress details (nature, position, time), the
  frequencies offered, the end of sequence (ack RQ/BQ), and whether the
  ECC was right. The station list has the six distress and safety
  channels (2187.5 to 16804.5 kHz).
* **Tests**: the code against M.493's Table 1, calls with errors in one
  copy and in both (the ECC repairs them), a distress alert, a phasing
  mostly lost, and a skimmer test with four calls in audio. **On air**
  (the remote unit's bins, 2026-11-16 10:13-10:23 UTC, 6312, 8414.5 and
  12577 kHz): 8 calls, all with a right ECC: ships' test calls to coast
  stations and the coast stations' acknowledgements (one offering a
  working frequency).

### 2.9 Time codes (new): done (2026-11-16)

`crates/timecode`: DCF77, MSF, WWVB and JJY (a keyed LF carrier heard as a
tone in USB/LSB/CW) and WWV/WWVH (the 100 Hz BCD subcarrier, in AM),
written from the stations' published formats (PTB, NPL, NIST SP 432,
NICT). The Time panel (in the Decoders list) picks the station the tuning
hears, or tunes one (USB with the carrier at 1 kHz; AM on WWV).

* **The tone**: mixed to 0 Hz, a 40 ms boxcar (~25 Hz: on the unit's HF
  antenna DCF77 is ~22 dB over the noise in 1 Hz, so about 0 dB in 200
  Hz), its amplitude every 5 ms; impulses (6x the average) blanked; the
  envelope divided by its up level over the 2 s around it (the AGC
  recovers from an impulse over seconds, which otherwise reads as keying).
* **Seconds**: the last 20 s folded into one second and matched against
  the station's average keying gives the phase (moved only when clearly
  better, or by a few frames); each second's mean level in the station's
  windows, scaled by the 30 s levels, nearest the symbols' templates (a
  marker costs a little more: it is rare).
* **Minutes**: until locked, every alignment of the last seconds whose
  minute passes all checks (parities, markers, ranges); then counted (a
  stray marker-like second doesn't move it), dropped after two failed
  minutes. The date fields are voted over the last ten minutes (two errors
  in DCF77's date pass its one parity bit); a minute is confirmed when its
  time of day agrees with an earlier passing one. The panel shows the
  station's time, UTC, the checks, and when the minute began by the
  server's clock less what it says (the path from the station, the
  receiver and the server's clock).
* **On air** (the unit's LF bin, 2026-11-16 10:13 UTC, 10 min): DCF77
  ~9 % of symbols misread (impulse noise sets it; projecting on the
  carrier's phase, or scaling by the last seconds' levels, read more
  wrong); minutes pass every few minutes, 17 ms after the recording's
  clock; the page's AGC'd audio (replayed) reads alike since the up-level
  scaling. MSF can't be judged on this unit: a steady 60 kHz line (the
  bins' 60 kHz frame rate?) fills its off periods. WWV's carriers came in
  (10 MHz 32 dB in 1 Hz) but its 100 Hz code wasn't over the noise.
  WWVB, JJY and WWV are tested on synthetic keying only.
* **Not done**: RBU, BPC; WWV's coherent subcarrier. (CHU's 300 Bd FSK
  code: none; CHU left shortwave on 2026-06-22.)

### 2.10 DGPS beacons (new): done (2026-11-28)

Maritime DGPS (IALA beacons, 283.5-325 kHz, ITU-R M.823): MSK at 100 or
200 Bd (50 allowed) carrying RTCM SC-104 version 2. Asked for after the
RTTY panel left 287/311/323 kHz alone (MSK, 50 Hz shift: below its 60 Hz
pairing limit, and no text inside). `crates/dgps`, `dgps.wasm`, the DGPS
panel.

* **Finding beacons**: MSK squared gives two lines a bit rate apart,
  centred on twice the carrier (carriers and NDBs square to one line).
  Every 2 s (4 s on a 60 kS/s bin) the last 8 s: one FFT of the history,
  pieces of 3 kHz cut from it and squared (wider pieces raise the
  squared floor: a 6 kHz piece lost a weak 200-Bd beacon at 286.5 kHz
  that 3 kHz found), pairs 50/100/200 Hz apart both 15 dB over the floor.
  A candidate is kept once 3 words in step with a header pass parity.
* **Demodulator**: 12 samples a bit (the channel rate divides 12 kS/s
  and 60 kS/s for 50/100/200 Bd), a ±0.75 × rate FIR (neighbours are
  500 Hz away: a wider filter let them in and most beacons failed in
  the prototype), the bit from the sign of the phase turned over one bit,
  timing where |turn| is largest, a slow loop on −turn².
* **RTCM**: 30-bit words (24 data + 6 GPS parity, D29*/D30* chained; an
  inverted stream reads the same), header (type, station, Z-count,
  sequence, length, health); decoded: 1/9/31/34 (each satellite's
  pseudorange correction, its rate, UDRE, IOD), 3 (reference station,
  ECEF to WGS84), 7 (beacon almanac), 16 (text), 36 (GLONASS text,
  KOI8-R).
* **On air** (two 5-min 60 kS/s bins, 279-351 kHz, 2026-11-28 00:33 UTC,
  `/data/misc/web888-iq/2026-11-28_dgps/`; `--example replay`): 27
  beacons, the strong ones 95-100 % of messages whole. Positions check
  out (311.0 kHz at 59.99 N 29.13 E, 289.0 at 63.48 N 19.65 E), the
  almanacs list the Finnish beacons (Porkkala 293.5 kHz), corrections
  agree across stations (GPS 3: −5.1, −6.5, −5.5 m from three beacons).
  Some beacons come and go (287.5 kHz half the time). The page's audio
  path (the same minutes as USB audio) reads what the IQ path reads.
  Replay cost: 300 s of the bin in 22 s on the desktop.
* **Page**: DGPS in the Decoders list; per beacon its station, health,
  words passing parity, position and distance/bearing from the
  receiver's locator, message counts by type, texts, the almanac, and a
  corrections table (satellite, correction m, rate cm/s, UDRE, the last
  15 minutes plotted). Wide: the 40 kHz bin around the dial (280-320 kHz
  from a 299.4 kHz dial: 11 beacons live). A wasm32 bug found there:
  `len * m` overflowed usize at a 60 kS/s history (fine natively).
  `ui/tools/cdp/dgps.mjs` (WIDE=1).
* **Not done**: the map (beacon positions as points), station names (an
  ID list; the almanacs give positions), type 5/27 contents.

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
trained on), character errors (`--example cer`; the current weights, after
the pile-up fine-tune of §5.2):

| SNR | clean | 10 % jitter | QSB 0.2 Hz |
|---|---|---|---|
| +6 dB | 0.0 / 0.0 % | 2.2 / 0.0 % | 9.8 / 1.0 % |
| 0 dB | 0.0 / 0.0 % | 2.4 / 0.0 % | 26.2 / 6.4 % |
| −3 dB | 0.4 / 0.0 % | 2.2 / 0.0 % | 38.1 / 10.4 % |
| −6 dB | 10.2 / 0.2 % | 16.2 / 0.0 % | 57.2 / 21.0 % |
| −8 dB | 57.8 / 0.2 % | 61.7 / 0.8 % | 67.2 / 29.0 % |
| −10 dB | 80.8 / 2.3 % | 80.3 / 6.0 % | 78.4 / 39.2 % |

Since 2026-11-23 the synthesiser's fading comes from ionochannel
(Watterson, a Gaussian Doppler spectrum) instead of a 16-sinusoid Jakes
sum, so the QSB column is now a 0.3 Hz spread (2σ; about the same RMS
Doppler as Jakes at 0.2 Hz): +6 dB 10.2 / 1.8 %, 0 dB 19.4 / 5.4 %,
−6 dB 54.1 / 16.0 %, −10 dB 73.8 / 30.8 %. The other columns are
unchanged to the bit.

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

### 5.2 Pile-ups and fading (2026-11-15)

`--example pileup`: a running station at 0 Hz and a pool of 2-4 callers
(22-35 WPM, within ±6 dB of each other) answering every round, one worked
and replaced each round, for 60 s of 12 kS/s IQ; callers spread ±40,
±100 or ±300 Hz; each station fading on its own (`FADE=none|qsb|aurora`:
Rayleigh at 0.05-0.5 Hz or 2-15 Hz). Scored on the callers' own sending
(the runner's exchange leaves their calls out): the share of callers whose
call is read as a word of its own, and the callsign-like words that are no
station's call. The first version let the runner echo the worked call,
and ended the run without letting the last 3 s print: both flattered the
numbers.

What failed, in order:

1. **The runner's channel read its callers.** The network re-centred each
   8 s window on the loudest carrier within ±150 Hz, and the classical
   decoder searched ±40 Hz around the last estimate, so a louder caller
   took the channel over (and a caller's channel walked towards the
   runner run by run). Now the carrier is searched within ±12 Hz of where
   the signal was found (`ANCHOR_HZ`) and the network centres within
   ±8 Hz of that. On the 40 m contest capture this alone cut words run
   together from 1.2 % to 0.3 %.
2. **Callers got no channel.** Channels were at least 70 Hz apart, the
   detection spectrogram (43 ms) can't resolve carriers closer than ~45 Hz,
   and a line counted as keyed by its 90th percentile level, which a
   caller keyed a few % of the time never reaches. Now peaks come from a
   second, 1024-point spectrogram (11.7 Hz lines), channels may be 20 Hz
   apart (`spacing`), and the keyed level is the 99th percentile
   (`hi_pct`). The finer peaks found hard keying's click sidebands as
   signals (70 spurious channels among 24 stations in `--example wide`):
   a peak 40-300 Hz from one 10+ dB stronger whose level correlates with
   it (> 0.3) is dropped.
3. **The network had not learnt to ignore close neighbours.** Training
   now has pile-ups on 30 % of examples: 1-3 other signals 12-120 Hz away
   at −6..+10 dB, keyed over the wanted one, the label only the centre
   signal (15 % with the centre silent). A 20k-step fine-tune (30 min):
   validation error on pile-ups 61 % → 2.9 %, the other bands as before
   (−12..−6 dB 23.5 → 22.2 %).

Callers read (QSB, 24 trials):

| | tight ±40 Hz | medium ±100 Hz | wide ±300 Hz |
|---|---|---|---|
| before: calls read | 15 % | 22 % | 54 % |
| before: no-call words | 53 % | 65 % | 46 % |
| after: calls read | 42 % | 60 % | 80 % |
| after: no-call words | 24 % | 12 % | 6 % |

The fine-tune hardly shows here (the old weights read about the same with
the skimmer changes): what's left is mostly callers within ~15 Hz of the
runner or of each other calling at once, which no channel separates. On
the 40 m capture the skimmer now follows 35 signals instead of 20 and
reads 445 words instead of 316: callers next to running stations decode
on their own, words run together 0.4 %; a few garbled stations are split
in two. Cost (`--example wide`, WebAssembly): 42 % classical, 58 % neural
of a core for 24 stations in a bin (was 36 % and 46 %).

**Fading and auroral flutter.** ITU-R F.1487 models HF fading as
Gaussian Doppler spectra; its high-latitude spreads (2σ) are 0.5 Hz
quiet, 10 Hz moderate, 30 Hz disturbed (25 Hz exceeded 5 % of the time;
mid-latitudes 0.1-1 Hz). Measurements on auroral paths report 2-55 Hz,
more in auroral backscatter. A real recording (an Alaska station on
18 MHz heard in Norway at Kp 2) measures ~6 Hz (from the decay of the
carrier's autocorrelation over key-down samples, checked on synthetic
fading of known spread: it reads 10-20 % low).

`--example flutter`: one signal, 5 % jitter, Rayleigh fading with a
Gaussian Doppler spectrum (`synth::Channel::spread_hz`). The classical
decoder fails from 3 Hz on even at 20 dB (its fit assumes a steady
level). Training (`synth.py`) now has flutter on 15 % of examples:
3-30 Hz (log-uniform), half of them two modes up to 10 Hz apart, the
neighbours fluttering too (and every neighbour fading on its own); a
20k-step fine-tune from the pile-up weights: validation error with
flutter 19.8 % → 2.1 %, the others as before (pile-ups 1.4 → 1.6 %,
−12..−6 dB 19.8 → 19.0 %). Character errors, classical / neural, 20 WPM:

| SNR | 3 Hz | 10 Hz | 20 Hz | 30 Hz |
|---|---|---|---|---|
| 10 dB, before | 74 / 2.0 % | 72 / 3.3 % | 51 / 22 % | 34 / 32 % |
| 10 dB, after | 74 / 0.2 % | 72 / 0.0 % | 51 / 0.2 % | 34 / 4.3 % |
| 0 dB, before | 76 / 19 % | 74 / 29 % | 50 / 42 % | 36 / 58 % |
| 0 dB, after | 76 / 7.9 % | 74 / 2.5 % | 50 / 1.6 % | 36 / 4.1 % |

(Strong flutter reads better than 3 Hz at 0 dB: at 3 Hz the fades last a
character or more, at 20 Hz they average out within a dot.) Pile-ups with
every station fluttering (3-20 Hz, `FADE=aurora`): calls read 21 / 39 /
50 % → 33 / 64 / 76 %, no-call words 49-59 % → 10-37 %; with QSB as
before. The real recording decodes the same ("TU L7SB UP" without
spaces, its first letter cut by the clip). Still to do: check against
on-air captures during an aurora.

### 5.3 Signal identifier: done (2026-11-15)

`crates/sigid` (features, inference without a framework, the passband
scanner; 529k parameters as f16, 1.1 MB), `crates/sigid-wasm`
(`sigid.wasm`, 1.5 MB, loaded only when used), `tools/ml/sigid/`
(training, `README.md`), the ID button and its panel beside the waterfall.

* **Classes** (18): noise, carrier, CW, 2-FSK (RTTY 45-75 Bd, SITOR-B /
  NAVTEX, ASCII, DWD 50/450: the RTTY panel works out which), PSK31/63/125,
  FT8, FT4, WSPR, ALE, STANAG 4285, MIL-STD-188-110A, SSTV (13 modes), HF
  fax, SSB voice, AM, local interference, OTH radar, HFDL (since
  2026-11-16). A row's Decode opens the page's decoder (CW, RTTY, PSK,
  fax/SSTV, FT8/FT4/WSPR, HFDL: the nearest listed channel); Tune tunes to
  voice (LSB below 10 MHz) or AM; interference and radar are only named.
* **Input**: 4.096 s of complex baseband at 6 kS/s, the signal at the
  centre. Three views: a two-resolution spectrogram (43 ms and 11 ms
  windows on one grid), a 1.46 Hz periodogram (FT8 and WSPR tones), and
  |autocorrelation| at 1-400 ms (frame periodicities: STANAG 4285 and
  MIL-110A look the same in a spectrogram). Model: a 2-D CNN pooled over
  time but not frequency (the label is the signal at the centre; its
  neighbours are elsewhere), 1-D CNNs on the other two views, two linear
  layers.
* **Training data**, made on the fly (`modes.py`, `data.py`): content random
  where it doesn't identify the mode, exact where it does (Costas arrays,
  frame, probe and scrambler sequences, baud rates, shifts, SSTV line
  timings); speech from espeak-ng (~40 languages) with an on-air spectral
  tilt. Channel: offsets (voice and AM anywhere in their band, as the
  scanner sees them), QSB or auroral flutter, multipath, neighbours of
  other classes, birdies, impulses, −6..+30 dB, passband edges (AM cut by
  an SSB filter too), LSB inversion. 20k steps from scratch, then two
  fine-tunes (10k and 8k), ~2.5 h on 16 threads + an RTX 5070 (the data
  generation limits it).
* **In the page**: the worker keeps 4.1 s of the 12 kHz audio; every 2 s the
  scanner finds the signals in the passband (power over the floor
  smoothed over 120 Hz, gaps up to 190 Hz joined) and identifies the next
  one: an FFT of the audio, an inverse FFT of the 6 kHz around the signal
  (no image; LSB flipped back). ~0.3-0.5 s of a core in WebAssembly each
  (0.54 s measured while training loaded every core; 0.14 s natively);
  each signal's probabilities are averaged over its runs.

Results. Synthetic validation (1500 windows): 97.7 % (93 % at −6..0 dB).
Real windows labelled by hand from waterfalls (`real.py eval`):

| recording | windows | right | misses |
|---|---|---|---|
| 40/30 m, 12 min (2026-10-23) | 506 | 82 % | mostly nothing at the centre then (FT4's 7.5 s cycles, pauses in QSOs and CW, the fax's blank phases); weak SSB QSOs called MIL-110A |
| 40 m, 60 s (2026-10-22) | 60 | 97 % | two weak QSO windows called noise |

DWD RTTY 48/48, six AM broadcasters 137/144, both FT8 sub-bands 38/48,
fax 21/24; the scanner on 20 m FT8/FT4 corpus slots: 17/17 FT8 and 9/10
FT4 regions right. In the page (the 60 s recording replayed): FT8 on
7074 kHz 78-99 %, DWD fax 86 %, DWD RTTY 95-98 %, AM heard in USB 70 %.
Weakest: SSB voice. espeak speech isn't on-air speech, and a weak voice
shows only its lowest few hundred Hz to the scanner; LSB QSOs come out as
voice at 20-40 % (top, unsure) or as CW.

**Retrain (2026-11-16).** Run over four 10-minute wideband recordings of
2026-11-15 (80, 40, 20 and 15 m; 1404 windows as the scanner finds them,
`/data/iq/sigid-wide`), the 15-class model was right on STANAG 4285 and the
ham signals, but a third of the windows were local interference it called
fax, SSTV or MIL-110A, an OTH radar had no class, and HFDL bursts were
MIL-110A. Three classes were added with generators modelled on those
windows (the real ones kept as a test set, so as not to learn one site's
interference), and 803 SSB clips from the 40 m phone segment
(`onair.py`) joined the espeak speech. 30k steps from scratch, then two
fine-tunes of 10k after the generators were matched to what the real
interference showed: carriers with phase-noise skirts, pairs with the
window on one line, and their regular beat (wideband ticks every 0.405 s
on 20 m, skirts pulsing every 0.1-0.14 s or 1.1 s on 80 m), which the
model had taken for fax's 0.5 s lines.

| test | 15 classes | 18 classes |
|---|---|---|
| synthetic validation | 97.7 % | 96.9 % (of 18; 93 % at −6..0 dB) |
| HFDL bursts the decoder found (6646 and 8942 kHz) | 0/88 (MIL-110A) | 83/88 (5 interference) |
| STANAG 4285 windows with its 106.67 ms frame | 148/169 | 148/160 |
| windows called MIL-110A without its 20 ms frame | 100 | 22 |
| 80 m interference comb (535 windows) | 0 interference, 255 fax | 246 interference, 101 fax |
| AM in USB audio, carrier at 300 Hz (three stations) | 13-46 % | 55-85 % |
| AM in USB audio, carrier at 1500 Hz | 52-88 % | 70-92 % |

Left: the 20 m carrier pairs (~610 Hz apart with ticks every 0.4 s) are
still fax (76 of 303 windows); two steady tones and a regular tick are
what a black-and-white fax looks like at this resolution. Weak SSB in USB
audio is still unsure (20-35 %).

### 5.3.1 "What is this?" (2026-11-23, phase 1)

A closer look at one signal, explained in a few sentences: shift-drag
(or shift-click) across it on the waterfall, or its **?** in the ID
panel. The receiver is retuned (the passband widened, up to 5 kHz) when
the range isn't in the audio; 4.1 s of audio later the panel shows a
headline (the mode, how sure), what the signal looks like and why, where
it is (the band plan's band or segment, the labels near it), Decode/Tune,
and the measured figures.

* **Measurements** (`crates/sigid/src/measure.rs`, deterministic DSP on
  the scanner's 4.096 s of audio): the floor (10th percentile of an
  85-ms spectrogram over time and frequency, so speech pauses and gaps
  between bursts count; `fills` when the signal covers the passband and
  no floor can be seen); the extent over the floor, centre, 90 % power
  width and SNR; keying (time on, bursts, shortest elements), level
  variation in 100-ms blocks (fading left out) and its share at syllable
  rates; tones (peaks with 6 dB prominence) and their grid; the strongest
  line's share and the sideband power on each side of it; the symbol
  rate from lines in the envelope's spectrum (PSK, filtered modems) or in
  the transitions of a two-tone discriminator ((a − b)/(a + b) of each
  tone through a moving average of 1/shift: selective fading cancels)
  or the instantaneous frequency (MFSK), with the shortest runs deciding
  the multiple (async RTTY's 1.5 stop bits put its transition line at
  twice the rate); the PSK order from the lowest power of the signal
  that shows a line; repetition periods of the signal (|R|, beats of its
  tones excluded) and of its envelope; drift.
* **Text** (`ui/src/sigdesc.ts`, pure, tested on real analyses in
  `ui/test/fixtures/sigdesc.json`): features from the figures (carrier,
  keyed, AM, carrier with one sideband, speech, two-tone FSK, MFSK, PSK,
  pulses), matched against known signatures (RTTY rates and shifts, the
  DWD's 50 Bd/450 Hz, SITOR-B, ASCII, fax's 800 Hz, 2G ALE, HFDL, 2400 Bd
  serial-tone, STANAG 4285's 106.7 ms frame, PSK31/63/125, CW speed),
  then checked against the identifier's top two: agreeing is "the figures
  fit it"; an exact match it didn't name is said, with its guess; a guess
  the figures contradict (AM without a carrier, a carrier that is keyed)
  is said not to be borne out. A matched mode states its own symbol
  rate, or none (fax, CW); a measured rate that doesn't fit is left out.
  Through i18n (fi, sv, de).

Results. Synthetic signals from `modes.py` (three seeds at 15 and 3 dB
in 2500 Hz): RTTY/SITOR rate and shift right on all six, PSK31 and
PSK125 rate and BPSK, CW ~20 WPM, HFDL 1800 Bd, MIL-110A 2400 Bd,
STANAG 4285's frame at both SNRs, ALE's 250 Hz grid (the wide ones in a
100-4900 Hz passband; in 200-2800 Hz they fill it, and say so). On air
(the 60 s recording of 2026-10-22, in the page): DWD 10100.8 kHz "RTTY
50 Bd, 450 Hz shift" with 443-446 Hz measured through selective fading;
AM broadcasts (carrier with both sidebands; heard in USB with the carrier
at 300 Hz: one sideband, "AM heard in SSB?"); 20 m SSB QSOs the identifier
called AM: no carrier, syllable-rate level, "Speech (SSB voice)".

Left (next phases): wide signals need wider input than the audio (OTH
radar, DRM, broadcast AM's both sidebands: IQ from the zoom or bins
path); new classes (DRM, Olivia/Contestia/MFSK, PACTOR/Clover/VARA, Hell,
3G ALE); calibrated "not sure" from the identifier; more real labelled
windows. `measurewav` (crates/sigid) and `anjson` (crates/sigid-wasm)
examples print the figures and the page's JSON for a WAV.

### 5.3.2 Phase 2 (2026-11-23): wide look, more modes, radars

* **Wide look**: the channel's IQ (±10 kHz at 24 kS/s where offered,
  else ±5 kHz) for 4 s, then the tuning back (`measure_iq`,
  `analyse_iq`, `sig_iq_*`); for ranges wider than the audio and from
  every answer's Wide look button.
* **More figures**: the tone grid frame by frame (MFSK, parallel
  carriers), FSK shift from the frequency track, noise-corrected
  kurtosis (1 constant envelope, ~1.3 single-carrier PSK, 2 OFDM), the
  OFDM symbol in |R|, keying depth, AM's mirrored sidebands, carriers
  side by side, sweeps (slope along each burst or between flybacks, the
  burst length, the rate), a noise floor from 85/21/5 ms frames (a sweep
  lights all of a long one).
* **Named by their figures**: DRM (mode from its symbol), Olivia/Contestia,
  MFSK8-64, DominoEX/THOR, Feld Hell, PACTOR, MT63, the 39-tone modem,
  3G ALE, Codan data and CHIRP, CLOVER 2000, Chinese-64 MFSK, CIS-36-50,
  F01/F06, F11 (sigidwiki.com's figures, "fits ..."); radars and sounders
  by pulsed or FMCW, sweeps a second, sweep width (slope x pulse, or slope
  / rate) and direction: 29B6 'Kontayner' and its sounder, OTH-SW,
  Podsolnukh, ROTHR, JORN, Chinese 'Foghorn' and 160 kHz radars, chirp
  ionosondes (several fitting: all named).
* **On air**: the 2026-10-22 recording's labelled windows 18/18 (DWD
  RTTY, AM stations fading or with little modulation, FT8 crowd, weak
  WSPR, weak LSB speech); the OTH radar on 7000 kHz (2026-11-15) "fits 29B6
  'Kontayner'" in all three windows: 4 ms pulses 40 a second sweeping up at
  3.51 MHz/s, ~14 kHz. A wide look on 7265 kHz in the page: two AM
  stations side by side, both labels named.
* Seven classes added to the identifier's generators (olivia, mfsk,
  dominoex, hell, pactor, multitone, ale3g); `calibrate.py` for its
  probabilities.
* **The 25-class model (2026-11-23)**:
  * Fine-tuned from the 18-class one: 20k steps, 3.7 h, `--resume`
    growing the last layer.
  * Synthetic validation 97.5 %: 95.8 % at −6..0 dB, 98.4 % at
    +15..+30 dB, every class 92 % or better (AM 91.7 % the lowest).
  * Temperature 0.653 folded into the last layer: expected calibration
    error 4.7 % → 0.75 %, so "90 %+" is right 99 % of the time.
  * Real signals:
    * The labelled windows from the 2026-10-22 recording are 18/18 (one
      fixed in the text layer: a fading AM carrier the model called
      WSPR, then fax; fax now needs its ~800 Hz black/white shift).
    * The model now names the new modes itself, where it said fsk, cw,
      mil110a or interference before; their answers went from "likely"
      to "sure".
    * SSB voice is ssb 0.66, where it was am 0.57.
    * A plain carrier is carrier 0.92, where it was am 0.50.
    * Weak LSB speech is still am 0.86, but the figures overrule it.
  * Fixed along the way: false "sweeps" on wideband modems (STANAG 4285,
    HFDL, fax, the 39-tone and Codan modems, 3-14 short rough glides).
    A repeated sweep now needs 8+ alike bursts with median r² ≥ 0.95;
    every radar here has 41+ at 0.996+.
  * Also fixed: a sweep can't be wider than the signal it was measured
    in (a 206 Hz AM carrier's "4 kHz" glide in the page).
  * Open: the DWD RTTY reads 100 Bd in 1 of 12 windows of the recording
    (t = 0, in a fade; 50 Bd in the other 11).

### 5.4 Neural noise reduction: voice, CW, AM broadcast (SSB voice in the page 2026-11-24, CW and AM 2026-11-25)

After the 25-class identifier. Small mask-only networks in the page's
audio path (AudioWorklet, our own Rust inference like §5.1), as a third
choice next to the server's spectral subtraction; no load on the unit.
One model per kind of listening, picked by the mode: a voice model
trained on speech treats a steady tone as noise and fades CW, carriers
and data tones away (a known RNNoise trait). For listening only: the
decoders keep taking the raw audio, which they read best.

| mode | model | off for |
|---|---|---|
| USB / LSB | SSB voice (below) | — |
| CW | CW (below) | — |
| AM, SAM/SAL/SAR | AM broadcast, speech and music (below) | — |
| anything else (data modes, FM, IQ) | none | — |

**Papers (searched 2026-11-23):**

* RNNoise, Valin 2017/18 ([arXiv 1709.08243](https://arxiv.org/abs/1709.08243)):
  a GRU predicts ~22 Bark-band gains, a pitch comb removes noise between
  the harmonics; 1.3 % of an x86 core; BSD; v0.2 trained on public data
  only. The SDR baseline (SDRangel's denoiser, the extended WDSP used by
  Thetis/piHPSDR); SDR-Console tests a DNN of its own.
* PercepNet, Valin et al. 2020 ([arXiv 2008.04259](https://arxiv.org/abs/2008.04259)):
  RNNoise plus a better comb filter, clearly better, 4.1 % of a core, 40
  ms look-ahead.
* DeepFilterNet 1-3 ([2110.05588](https://arxiv.org/abs/2110.05588),
  [2305.08227](https://arxiv.org/abs/2305.08227)): ERB gains for the
  envelope + a short complex "deep filter" in the low band; DF3 real-time
  factor 0.19 on one notebook thread; the quality reference, built for
  48 kHz and larger than we need.
* Very small models: GTCRN (ICASSP 2024; 24k weights, 48k with the fixed
  ERB layer, ~33-40M MAC/s, streaming, beats RNNoise), UL-UNAS
  ([2503.00340](https://arxiv.org/abs/2503.00340)), CoFi-Lite
  ([2607.10142](https://arxiv.org/abs/2607.10142), 2026: 83k weights,
  12.9M MAC/s, DNS3 PESQ 2.16 vs GTCRN 2.07), LiSenNet, AdaptCRN; DTLN
  ([2005.07551](https://arxiv.org/abs/2005.07551), < 1M weights, two LSTM
  stages, real time on a Raspberry Pi 3). Code public for GTCRN, UL-UNAS,
  DTLN.
* HF: "A Hybrid Deep-Learning Approach for Single Channel HF-SSB Speech
  Enhancement" (IEEE WCL 2021, [9476003](https://ieeexplore.ieee.org/abstract/document/9476003/)):
  an anti-fading CNN first, then noise suppression; fading first recovers
  more weak speech. The Paderborn HF speech database
  ([2106.02472](https://arxiv.org/abs/2106.02472)): LibriSpeech sent in
  LSB from Paderborn and received by KiwiSDRs across Europe, the clean
  source time-aligned, SNR −20..+5 dB, ~23.5 h (3.5 h speech; companion
  [2103.01599](https://arxiv.org/abs/2103.01599) also estimates SSB
  mistuning from pitch). Licence not checked yet.
* Hallucination: generative enhancers (diffusion, flow matching,
  WavLM-based) invent spectral content below about −7 dB
  ([2606.02913](https://arxiv.org/abs/2606.02913)); DNSMOS doesn't
  notice ([URGENT 2025](https://arxiv.org/abs/2505.23212),
  [PASE](https://arxiv.org/abs/2511.13300)); training for DNSMOS sounds
  worse to listeners ([2403.11732](https://arxiv.org/abs/2403.11732)).

**Plan:**

* Model: GTCRN/UL-UNAS class (~25-80k weights), causal, mask only (it
  can take sound away, never add: no guessing), plus an RNNoise-style
  pitch comb; at 12 kS/s (24 kS/s variant later), f16 weights ~50-160 kB.
* Training data: LibriSpeech / Common Voice speech mixed with real HF
  noise from our recordings (`/data/iq`, the identifier's `onair` set);
  simulated SSB passband 300-2700 Hz, mistuning ±150 Hz, selective
  fading (the HF-SSB paper's point), impulses (QRN), carriers and CW
  (QRM), AM too.
* Scoring: the Paderborn database and our own SSB/AM recordings; Whisper
  word errors (local whisper.cpp) plus PESQ/STOI from −10 to +10 dB,
  against `nr.rs` and RNNoise; listen below −7 dB for invented sound.
* Effort: about the neural CW's; the data simulation is most of it.

**SSB voice: first model in the page (2026-11-24).** `tools/ml/nr`
(training, scoring), `crates/nr` (streaming inference, matches PyTorch),
`crates/nr-wasm` and the **NN** button beside NB/NR (USB/LSB, 12 kHz
audio; a strength slider; the decoders keep the raw audio). The first
model (run 5): a causal 256/128 STFT, conv + 2 GRU (96) + per-bin gains,
161k weights (315 KiB f16), 21 ms latency, ~15 M multiply-adds/s;
replaced by run 6 the same day (below). Training data:
LibriSpeech through a ham transmitter chain (EQ, compressor, clipper),
ionochannel's HF path and receiver, and real KiwiSDR noise from the
Paderborn train split (half the batches); SNR −20..+15 dB; batches made
on the GPU (30k steps in ~35 min).

What was learnt on the way, on Paderborn's eval receptions (200, 1000
utterances; median in-band SNR −4.8 dB):

* Synthetic noise only (runs 2-3): every recogniser read the processed
  audio worse than the raw (Whisper 52 → 61-70 % WER at full strength);
  ESTOI fell. More low-SNR training and a heavier penalty for removed
  speech didn't change that.
* Real noise in training (run 4): much closer (Whisper 61 %, Parakeet
  51 %, Vosk 75 % at full strength; at strength 0.8: 53 / 40 / 69 %
  against raw 52 / 38 / 71 %); STOI up a little, ESTOI level. Machines
  trained on noisy speech gain little from a mask; the fragile one
  (Vosk) gains.
* Paderborn's voices are audiobook reading without a transmitter's
  processing, unlike real SSB (the user, listening). Run 5 trained on
  processed speech; on real 40 m phone (onair40) the user preferred it
  over run 4, at 0.8 and at full strength depending on the signal, so
  the strength stays the listener's.

Run 6 (the user found run 5 taking voice away where the noise is
strongest): 32 smooth bands instead of per-bin gains, a pitch feature
(autocorrelation over 60-400 Hz), a target keeping the noise 10-15 dB
down, a richer transmitter chain (mic EQ, narrower filters, heavier
compression); 265k weights. On Paderborn (`attn.py`: the gain in speech
frames and in pauses, per band) it keeps the voice (−5..−10 dB against
run 5's −9..−23) but takes only ~11 dB of noise: about what run 5 does
at 70 % strength, with a little more contrast below 1.5 kHz. By ear on
recordings the user heard little difference, so the comparison moved to
live signals: `nr-models.json` in the UI directory (none in a release)
lists weight files, and a selector beside the NN slider switches between
them (each model its own nr.wasm instance; `nr_load`).

**Run 6 ships (2026-11-24):** on live signals on the unit the user found
it clearly better than run 5. Built in since then (518 KiB f16, ~25 M
multiply-adds/s), at 100 % strength by default (its target already keeps
a noise floor).

**Run 7 ships (2026-11-24):** run 6's recipe with GRU 192 (539k
weights, 1.06 MB f16, ~55 M multiply-adds/s), 45k steps: +0.3-0.5 dB
SI-SDR on synthetic data, the same voice and noise gains on Paderborn
within a fraction of a dB; on live signals the user found it a little
better than run 6. Built in since then.

Run 8: run 7 fine-tuned with a speech-distortion weighted loss (the
gain on the speech alone against 1, on the noise alone against the kept
floor, the noise term × 0.5; `train.py --sdw`). On Paderborn it kept
~0.6 dB more voice above 1.5 kHz but took ~1 dB less noise and lost
contrast below 1 kHz (speech-to-pause 3.5 against 5.5 dB at 300 Hz);
synthetic SI-SDR 0.5-1.3 dB lower. Reweighting the loss moves the
trade-off along the same line: at these SNRs the model can't tell voice
from noise in those cells, so the next gains need what it sees (real
receptions, more context), not how it is scored.

Run 9: run 7 fine-tuned with half of each batch from real Paderborn
receptions (`pairbank.py`: 2776 train receptions, 100.8 h, median in-band
SNR −2.6 dB; the target the clean source delayed by the reception's lag
and through its estimated channel response). On the eval receptions it
keeps more voice and takes more noise below 600 Hz (speech-to-pause 8.1
against 5.5 dB at 300 Hz) but less contrast at 1-2 kHz; synthetic SI-SDR
as run 7.

Next: our own recordings with the unit, a 24 kHz path, a larger model
with more context, a noise-aware loss (weighing voice kept against
noise taken per bin by its SNR), fine-tuning on the Paderborn train
pairs.

**CW model** (for copying by ear):

* What a narrow filter and AGC can't do: soft-gate the noise between
  elements inside the passband, take lightning crashes down without
  chopping the keying, even out element levels through fading, and turn
  auroral flutter (a rasping smear) back into a steady-pitch tone that
  follows the measured envelope (a learned CW tone regenerator).
* It must not decode and re-key: a clean tone wherever it thinks a dit
  is would invent and drop elements at low SNR and sound sure of itself.
  So soft gains 0..1 per time-frequency cell with a floor (−10..−15 dB:
  weak elements still faintly heard, no dead silence), missed elements
  costing more than leftover noise in training; the flutter regenerator
  follows the envelope smoothly, never switching hard, so a doubtful
  element stays doubtful.
* Tiny (~10-30k weights: a few hundred Hz around the pitch). Training
  data from the CW synthesiser (§5.1-5.2: fading, jitter, QRN, a second
  station, auroral flutter); labels free.
* Scoring: synthetic sets at known SNRs, elements kept / lost / invented,
  raw vs processed; then human copying of the same recordings raw and
  processed (the neural CW decoder's errors on the output only a rough
  stand-in, as it trained on unprocessed audio).

**CW: first model in the page (2026-11-25).** The voice model's network
and stream (`crates/nr`) with CW weights: GRU 64, per-bin gains, no
pitch features, 83k weights (162 KiB f16), built into nr.wasm beside
the voice model (`nr_builtin`); the **NN** button uses it in CW mode,
with the same strength (to 300 %) and a selector for `kind: "cw"`
entries of nr-models.json. Not done yet: levelling fading and the
flutter regenerator (a mask can't add sound).

* Data (`tools/ml/nr/cwdata.py`, on the GPU, 0.25 s a batch of 64):
  keying from the CW decoder's synthesiser (QSO text, 10-45 wpm, jitter,
  dash ratio, Farnsworth), the wanted station at 300-1000 Hz and on 60 %
  1-3 others anywhere in the passband (−20..+10 dB); each faded on its
  own (none, QSB, auroral flutter on 12 %); QRN or (half) real Paderborn
  noise at −18..+20 dB in 2500 Hz, at times switching-supply hash,
  arcing, powerline bursts; a CW filter (200-500 or 500-1000 Hz) or SSB
  one, the AGC. Every CW signal is something to hear: the target is all
  of them plus the noise 10-15 dB down. Lost tone costs 4× (`--lost 4`).
* Run cw1, 30k steps (65 min): SI-SDR +1.9 / +2.8 / +4.0 / +3.0 dB at
  −18..−10 / −10..−3 / −3..5 / 5..20 dB. `cwscore.py` on the
  time-frequency cells, strength 1 (and 2):

  | SNR (2500 Hz) | tones kept | tone cells down > 6 dB | noise between |
  |---|---|---|---|
  | −18..−10 dB | −1.9 dB (−2.9) | 18 % (29 %) | −9.4 dB (−16.1) |
  | −10..−3 dB | −1.1 dB (−1.7) | 15 % (24 %) | −9.1 dB (−16.1) |
  | −3..+5 dB | −1.2 dB (−1.9) | 13 % (22 %) | −7.8 dB (−12.6) |
  | +5..+20 dB | −0.7 dB (−1.2) | 6 % (10 %) | −7.7 dB (−12.4) |

  The "down" cells are mostly element edges and cells where the tone
  barely stands over the noise, not whole elements; listening on live
  CW decides.

**AM broadcast model** (speech and music):

* Already handled elsewhere, so not its job: steady heterodynes and
  co-channel carrier beats (the auto notch, `crates/dsp/src/notch.rs`),
  selective-fading distortion and splatter from one adjacent channel
  (synchronous AM and its one-sideband modes, `demod::Sync`). The model
  sits after the demodulator and the notch.
* Its job: broadband hiss and atmospheric noise, crashes and clicks,
  powerline buzz and switching-supply hash, adjacent-channel "monkey
  chatter", drifting or wobbling whistles the notch doesn't catch.
* Music is the hard part: tonal music looks like carriers, and noise
  under it like music; mask models trained on speech make it sound
  "underwater". So trained on music as well as speech, with gentle
  gains and a strength control; never more than the hiss taken.
* Training data: clean speech and music → AM modulation (broadcast
  audio ~4.5 kHz, typical compression) → simulated HF channel (flat and
  selective fading, carrier fades, noise, interference above) → our own
  demodulator → input; target the clean audio band-limited the same.
  Music licences: first planned as CC0 / CC-BY only; decided
  (2026-11-25) that a mask model can't reproduce what it heard, so
  share-alike and no-derivatives terms (they bind copies and
  adaptations) don't apply to its weights, and all of MUSAN's music is
  used (Creative Commons or public domain, none non-commercial; the
  audio itself never leaves the workstation). A generative model would
  use the CC0 / CC-BY part only (`musicbank.py --strict`). Real AM recordings (our `onair`
  and `/data/iq` sets) for noise and for scoring, as they have no clean
  reference.
* Scoring: speech by Whisper word errors; music by listening (ViSQOL as
  a rough stand-in), against the auto notch + spectral subtraction.
* The largest of the three (music data and its evaluation); after the
  SSB voice and CW models.

**AM: first model in the page (2026-11-25).** The same network and
stream with AM weights: GRU 192, per-bin gains (narrow whistles),
pitch features, 558k weights (1.1 MB f16), built into nr.wasm beside
the voice and CW models; the **NN** button uses it in AM, SAM, SAL and
SAR (not SAS: two channels) with the same strength and a selector for
`kind: "am"` entries. nr.wasm (2.8 MB with all three) is fetched and
compiled once a page, an instance per model.

* Data (`tools/ml/nr/amdata.py`, at 24 kS/s complex on the GPU):
  LibriSpeech speech and MUSAN music (37 h, `musicbank.py`; 5.8 h of
  other artists held out): 40 % music, 40 % speech, 10 % speech over
  music, 10 % one turning into the other (a presenter between songs);
  broadcast processing (tilt, compressor, clipper, 2.5-5 kHz lowpass),
  30-100 % modulation, an ionochannel path or flat fading; QRN or real
  noise at −5..+25 dB, neighbours 9/10 kHz away, whistles, CW,
  switching-supply hash, arcing, powerline bursts; a DSB or one-sideband
  filter, the envelope or synchronous detector with the carrier AGC.
  The target is the same chain without the noise (the detector's
  fading distortion stays) plus the noise 10-15 dB down.
* Run am1, 45k steps (87 min): SI-SDR +6.0 / +5.2 / +4.7 / +3.0 dB at
  −5..3 / 3..10 / 10..17 / 17..25 dB (validation: held-out speech and
  music). Listening on live broadcasts decides; no music-specific
  score yet.

**Later, for fun: a generative voice model** (the user's idea,
2026-11-23; looked into 2026-11-25). A voice restorer that may invent a
little: weak SSB made to sound like a studio voice, at the price of now
and then saying what wasn't said. Only as a clearly labelled, separate
choice ("restore (may invent)"), never the default, never feeding the
decoders.

State of the art (searched 2026-11-25):

* Streaming flow matching: Stream.FM (Welker et al., Hamburg,
  [arXiv 2512.19442](https://arxiv.org/abs/2512.19442), TASLP 2026): a
  frame-causal NCSN++-style CNN (28M weights, 53M with its predictor),
  16 kHz, 32 ms algorithmic / 48 ms total latency (24 ms in a low-latency
  variant), frame-wise buffered inference that matches offline causal
  output with no recomputation; for denoising 1-2 network calls per
  frame are as good as 4-5. About 282 GFLOP per second of audio per call:
  real time on a desktop CPU only at one call. Code AGPL-3.0 (ideas only
  for us). Hsieh & Braun ([2510.16997](https://arxiv.org/abs/2510.16997):
  causal, 20 ms, ~5 steps as good as ~20) and Braun's mean flows
  ([2605.16251](https://arxiv.org/abs/2605.16251): one step, real time)
  go the same way.
* One step from bridges or adversarial training: few-step adversarial
  Schrödinger bridge ([2506.01460](https://arxiv.org/abs/2506.01460):
  one step beats a 50-step bridge, most at low SNR), SB-RF
  ([2606.05575](https://arxiv.org/abs/2606.05575)), VoiceBridge
  ([2509.25275](https://arxiv.org/abs/2509.25275)); DeepFilterGAN
  ([2505.23515](https://arxiv.org/abs/2505.23515): 3.6M weights, real
  time, GAN-based stochastic regeneration).
* StoRM (stochastic regeneration, Lemercier et al.; code MIT): a
  predictive model first, a generative one refines its output; fewer
  steps and fewer inventions than generation from noise alone.
* Hallucination: PASE (AAAI) separates linguistic (wrong words) from
  acoustic (another voice) hallucination; both grow fast at low SNR and
  are rarely measured there. Stream.FM adds a SpeechBERTScore loss
  against phonetic hallucination.

Plan, sized for the page (the published models are 100-1000 times too
heavy for wasm on one thread):

* StoRM-shaped on what we have: the shipped mask model's output is the
  starting point; a conditional flow (or bridge) from it, plus a little
  noise, to the clean speech, in the same causal 256/128 STFT at 12 kS/s
  (compressed complex spectrum), conditioned on the noisy spectrum and
  the mask estimate. Same latency (21 ms), same streaming code.
* Network: the mask model's family widened (causal conv + 2 GRU of
  ~384, complex velocity per bin), ~2M weights, ~200 M multiply-adds per
  second per call; 1-2 calls (Euler, or one step after adversarial or
  mean-flow training). The budget, timed 2026-11-25 (Node, this
  workstation, nr.wasm with SIMD128): the voice model (~50 M
  multiply-adds/s) takes 18.9 ms per second of audio, the CW model 4.2
  ms, i.e. ~2.7 G multiply-adds/s on one thread. So ~2M weights at two
  calls is ~15 % of a desktop core, likely 50-75 % of a phone's: two
  calls on desktops, one (or ~1M weights) on phones; WebGPU only if
  that isn't enough.
* Data: the voice model's (LibriSpeech through the transmitter chain,
  ionochannel, real Paderborn noise and the real pairs), clean speech as
  the target (no residual noise: this one may add).
* Scoring with hallucination first: on Paderborn's eval receptions by
  SNR bin, Parakeet and Whisper word errors against the reference text,
  insertions and substitutions apart (invented words), the mask model and
  the raw audio beside it; ESTOI; and listening below −7 dB for
  confident nonsense. A strength control mixes it with the mask output.
* After the AM model.

**Gen: the restorer in the page (2026-11-25).** `tools/ml/nr/gen.py`,
`crates/nr/src/restore.rs`, the **Gen** button in the Noise box (voice,
with NN on). As planned above, StoRM-shaped:

* Spectra divided by the noisy input's running level and compressed
  (c = Z |Z|^-0.5), real and imaginary parts side by side.
* Conditioning once a frame: the noisy features, the mask output's log
  power against the same level, the pitch features; a causal
  convolution over 3 frames to 256, two GRU layers of 256.
* Flow head once a step: [GRU output, the mask output's spectrum, the
  state, a time embedding] → 512 → 512 → 258 (SiLU). Path from the mask
  output plus 0.2 noise to the clean speech, straight; Euler, 1-2 steps.
* run 7's mask frozen (at floor 0); 1.81M weights; `nrgen.bin` 3.5 MB
  (f16), fetched when Gen is first turned on, not in `nr.wasm`.
* gen1: 60k steps, 2.5 h. Validation, SI-SDR improvement by input SNR:

  | SNR (dB) | mask | Gen, 1 step | Gen, 2 steps |
  |---|---|---|---|
  | −20..−10 | +4.1 | +7.6 | +6.3 |
  | −10..−3 | +5.4 | +8.2 | +7.7 |
  | −3..5 | +6.3 | +8.2 | +8.0 |
  | 5..15 | +6.3 | +7.5 | +7.4 |

  One step is better everywhere (the second step moves further from the
  mask output, towards a sample rather than the mean).
* Cost in `nr.wasm` (Node, this workstation): the mask alone 17 ms a
  second of audio, with Gen at 1 step 64 ms, at 2 steps 74 ms. The page
  takes 2 steps where the browser reports more than 4 cores, else 1.
* The strength slider mixes Gen with the mask output (below 100 %); the
  decoders never get Gen's audio. The Rust stream matches PyTorch
  (`restorer_matches_pytorch`, noise off, 2 steps).
* **On real receptions it fails** (`score.py --gen`, 200 Paderborn eval
  receptions, 1000 utterances; word errors, and substituted / deleted /
  inserted words per 100 reference words):

  | | raw | mask (NN) | Gen 1 step | Gen 2 steps |
  |---|---|---|---|---|
  | Whisper WER, all | 52.4 % | 52.5 % | 83.5 % | 85.3 % |
  | Whisper, raw WER < 30 % | 15.3 % | 17.7 % | 67.2 % | 72.3 % |
  | Whisper sub/del/ins | 30/17/5 | 30/17/5 | 35/44/5 | 31/51/3 |
  | Parakeet WER, all | 37.6 % | 38.9 % | 72.7 % | 77.9 % |
  | Parakeet sub/del/ins | 23/12/3 | 24/11/3 | 28/43/2 | 25/51/2 |
  | ESTOI | 0.341 | 0.351 | 0.202 | 0.161 |

  It doesn't invent words (insertions don't rise); it loses them:
  deletions 17 → 44-51 %, and the most even on the cleanest receptions
  (raw WER under 30 %: 15 → 67 %). The synthetic gain (+3 dB SI-SDR
  over the mask) doesn't carry over: trained on synthetic channels only
  (no real pairs, `--pairs`), it blurs real speech it hasn't seen.
  Gen is off by default; proposed (2026-11-26, the user to decide):
  hidden in releases until a model scores at least as well as the mask
  here.
* **Why, and the second round (2026-11-26).** `gendiag.py` (band power
  against the mask's output, in speech and in pauses): gen1 is
  23-26 dB under the mask in speech on real receptions, and 3-11 dB under
  clean even on synthetic data (the worst at low SNR). With 1-2 Euler
  steps the flow averages a phase it can't know, and the complex
  spectrum shrinks; SI-SDR is scale-invariant and hid it. Two changes,
  three runs of 40k steps each (`--mask run7`):
  `--mag` (flow on the compressed magnitude, the noisy phase kept, as
  the mask does) and `--pairs 0.5` (half of each batch real Paderborn
  pairs).

  | Gen, 1 step | Whisper WER | sub/del/ins | Parakeet WER | ESTOI | synthetic level vs clean |
  |---|---|---|---|---|---|
  | mask (NN), for reference | 52.5 % | 30/17/5 | 38.9 % | 0.351 | |
  | gen1 (complex) | 83.5 % | 35/44/5 | 72.7 % | 0.202 | −3…−11 dB |
  | gen2c (complex + pairs) | 90.6 % | 42/42/6 | 72.4 % | 0.151 | −3…−9 dB |
  | gen2mn (magnitude) | 80.8 % | 33/43/5 | 70.8 % | 0.242 | −1…−4.5 dB |
  | gen2m (magnitude + pairs) | **71.0 %** | 39/26/7 | **59.8 %** | 0.307 | −1…−4.6 dB |

  The magnitude form fixes the level on synthetic data, but on real
  receptions without pairs it is still about 22 dB under the mask in
  every band (the gap is between synthetic and real channels, not the
  phase). The pairs help the magnitude form (deletions 43 → 26 %) and
  hurt the complex one. gen2m, the best, against the mask by band
  (150-500 / 500-1k / 1-2k / 2-3.5k Hz): speech −8 / −16 / −30 / −37 dB,
  pauses −46…−65 dB. It keeps the low band and strips the consonant
  bands and the pauses (substitutions 30 → 39 %). None ships;
  `crates/nr/model` keeps gen1, and the Rust restorer reads both forms.
* **Third round (2026-11-26).** The real pairs' target (`pairbank.py`)
  is the clean source through each reception's measured response, and
  zero where the speech isn't heard over the noise: in 83 train
  receptions 10 % of 300-1000 Hz, 74 % of 1-2 kHz and 92 % of 2-2.7 kHz.
  Right for the mask (it can't bring back what's buried); a restorer
  taught it deletes the consonant bands. `pairbank.py --fill` fills the
  buried part of 300-2700 Hz with the median response measured
  (`/data/iq/nr/pairs-fill`, the same 2776 receptions). `gen.py --level
  L`: a hinge on the 1-step output more than 10 dB under the mask's
  output per band in the target's speech frames. Each from gen2m, 20k
  steps:

  | Gen, 1 step | Whisper WER | sub/del/ins | Parakeet WER | ESTOI | speech vs mask by band (dB) |
  |---|---|---|---|---|---|
  | mask (NN) | 52.5 % | 30/17/5 | 38.9 % | 0.351 | 0 |
  | gen2m | 71.0 % | 39/26/7 | 59.8 % | 0.307 | −8 / −16 / −30 / −37 |
  | gen3a (level, old pairs) | 73.9 % | 46/19/9 | 59.5 % | 0.298 | 0 / −3 / −5 / −6 |
  | gen3af (level, filled pairs) | 65.9 % | 40/19/7 | 53.6 % | 0.328 | 0 / −3 / −4 / −5 |
  | **gen3f (filled pairs)** | **65.0 %** | 35/25/5 | **53.2 %** | **0.348** | −8 / −14 / −19 / −20 |

  The filled target is the better half: fewer words lost without more
  wrong or invented. The level hinge brings the level up to the mask's,
  but what it brings back is wrong (substitutions 35 → 40-46 %,
  insertions up): loud isn't intelligible. gen3f is in
  `crates/nr/model` (server bd69b21) and on the unit for listening; still
  13 points behind the mask.
* **Plan, revised (2026-11-26, after a second opinion).**
  1. Diagnostics first (cheap):
     a. steps 1/2/4/8/16 on gen3f (40 receptions): 2 steps has always
        scored worse and quieter than 1, which a working flow shouldn't.
        Better with more steps: discretisation, distil to one step. No
        better: it's a shrinking regressor; drop the flow for a direct
        predictor;
     b. the real pairs' alignment: per reception, the frequency offset
        and the lag at the start against the end (pairbank.py models
        neither mistuning nor drift; a target 20-50 Hz or a few ms off
        teaches blur, i.e. substitutions);
     c. scoring as the page is used: gen3f blended with the mask at
        30/50/70 % (the strength slider), Vosk besides Whisper and
        Parakeet; gendiag against the clean target, not the mask (the
        mask's upper bands are mostly residual noise).
  2. The normaliser: `scale_of` (and Gen's mask-output feature) take the
     running mean of log power over all 129 bins, so the stopband's
     depth sets it: the gap between mean-of-log and log-of-mean is
     35.7 dB on synthetic batches and 21.7 dB on real pairs, and
     in-band compressed magnitudes come out ~14 dB lower on real data
     (p95 4.6 against 10.2). Level from the passband's mean power
     instead; a Gen-only change (the mask predicts ratios), mirrored in
     `restore.rs` with its own level. Fine-tune from gen3f and one run
     from scratch.
  3. From 1b: offset, lag drift and a slow gain track applied to the
     pair targets if they're off.
  4. From 1a: one-step distillation, or a direct predictor.
  5. Perceptual loss: a frozen speech encoder's features (WavLM or
     wav2vec 2.0) between the 1-step output and the clean target, in
     place of the level hinge (which made it louder, not clearer).
     Licence and download to be cleared with the user first. Whisper
     and Parakeet stay scorers only.
  6. The joint fine-tune with the mask; the mask retrain on `pairs-fill`
     last if at all (a mask can't bring back buried speech; a filled
     target mainly teaches it to pass noise there).
* **Pair alignment, measured (2026-11-26).** Mistuning is small: in 30
  train receptions the offset is −0.7…+3.7 Hz (p10-p90), none over
  20 Hz. But at SSB that turns the phase a few times a second, so the
  clean source correlates with the reception as a waveform at only
  0.08 (median): the real targets are good for magnitudes, useless for
  phase (why the pairs hurt the complex Gen). And pairbank.py's one
  waveform-correlation lag per reception is off: phase-free (band
  envelopes, `pairalign.py`) half the utterances sit 10 ms or more from
  it, some 40-60 ms (the global lag itself wrong), and the lag drifts
  within a file (online receivers' buffering and clocks): a frame or
  more of smear in the magnitude targets. `pairbank.py --align` places
  each utterance at its own envelope lag (`/data/iq/nr/pairs-fa`, with
  --fill). The mask run7 used no pairs, so it isn't affected.
  `gen.py --norm mean` takes the level from the log of the mean power;
  export.py refuses it until restore.rs reads it too.
* **Fourth round (2026-11-27).** Diagnostics on gen3f first:
  - Steps (Whisper WER, 40 receptions; the mask 49.1 %): 1 step
    61.9 %, 2 64.8 %, 4 70.1 %, 8 74.1 %, 16 77.4 %. Monotonically
    worse: a shrinking regressor, not a flow held back by
    discretisation. Step 4 is a direct predictor, not distillation.
  - Blends with the mask, as the strength slider makes them (1000
    utterances):

    | WER | mask | 30 % | 50 % | 70 % | 100 % |
    |---|---|---|---|---|---|
    | Whisper | 52.5 % | 53.5 % | 53.9 % | 56.5 % | 65.0 % |
    | Parakeet | 38.9 % | 40.0 % | 41.6 % | 44.4 % | 53.2 % |
    | Vosk | 68.7 % | **67.2 %** | 67.6 % | 68.7 % | 75.4 % |

    A light blend costs the big recognisers about a point and helps
    the small one: a low slider default is sensible.
  - Balance against 500-1000 Hz in speech (bands 0.3-0.5 / 0.5-1 / 1-2
    / 2-3.5 kHz): the clean source +3.2 / 0 / −5.1 / −9.1 dB, the raw
    reception +3.1 / 0 / −1.1 / −1.3, the mask +3.1 / 0 / −4.0 / −6.6,
    gen3f +6.5 / 0 / −8.1 / −14.5. Gen is ~3 dB light at 1-2 kHz and
    ~5 dB at 2-3.5 kHz against the clean source; the mask is a little
    heavy there (residual noise). (Corrected 2026-11-27: the first
    write-up took the raw row for the clean one.) Gen's band envelopes
    swing as much as the clean source's (std 22 / 20 / 18 / 15 dB
    against 19.5 / 18.3 / 16 / 15.7; the mask 9 / 7 / 5 / 4), so much
    of "quieter than the mask in speech" is the mask's residual noise
    in the gaps.

  Then three runs on the aligned pairs (`pairs-fa`: 2781 receptions,
  101 h; the alignment search vectorised, 33 min to build):

  | Gen, 1 step | Whisper WER | sub/del/ins | Parakeet WER | STOI | ESTOI | speech vs mask (dB) | balance (dB) |
  |---|---|---|---|---|---|---|---|
  | clean source | | | | | | | +3.2 / 0 / −5.1 / −9.1 |
  | mask (NN) | 52.5 % | 30/17/5 | 38.9 % | 0.552 | 0.351 | 0 | +3.1 / 0 / −4.0 / −6.6 |
  | gen3f | 65.0 % | 35/25/5 | 53.2 % | | 0.348 | −8 / −14 / −19 / −20 | +6.5 / 0 / −8.1 / −14.5 |
  | **gen4a** (gen3f + aligned pairs, 20k) | **60.9 %** | 33/23/5 | 48.9 % | **0.600** | **0.364** | −8 / −14 / −18 / −19 | +5.9 / 0 / −7.7 / −14.1 |
  | gen4n (gen3f + aligned, `--norm mean`, 20k) | 66.4 % | 37/23/7 | 51.6 % | 0.590 | 0.341 | −8 / −13 / −17 / −19 | +6.0 / 0 / −7.8 / −14.8 |
  | gen4s (scratch, aligned, `--norm mean`, 40k) | 62.9 % | 34/23/6 | **47.8 %** | 0.595 | 0.362 | −9 / −14 / −18 / −20 | +5.3 / 0 / −7.5 / −15.1 |

  The alignment alone gave 4 points on both recognisers and the first
  Gen above the mask on STOI/ESTOI. The passband level didn't touch the
  quietness or the tilt (gen4n and gen4s as quiet and as tilted as
  gen4a): the normaliser isn't the cause. Fine-tuning onto a changed
  normaliser costs (gen4n); from scratch it's level with gen4a. gen4a
  (log level, nothing to change in restore.rs) is in `crates/nr/model`
  and on the unit. `--norm mean` stays an option, not mirrored in
  restore.rs. Next: the direct predictor (step 4), whose loss can aim at
  the tilt and level directly; the perceptual loss (step 5) waits for
  the licence decision.
* **Fifth round (2026-11-27): the direct predictor and the feature
  loss.** The page ran Gen at 2 steps on desktops; it runs 1 everywhere
  now (better on every model, half the cost). `gen.py --direct` trains
  exactly that step, x0 + v(x0, 0) from the same noisy start, as a
  predictor (restore.rs unchanged):
  - the compressed magnitudes' squared error;
  - `--band`: each example's band energies (8 bands, 150-3500 Hz) over
    its speech frames against the clean target, two-sided |dB|/10.
    Per frame it doesn't work: band levels between formants swing tens
    of dB on residual noise, and half the 4-s pair windows have no
    speech (zero targets but for int16 rounding, +138 dB "errors");
    such examples (target 30 dB under the input) are left out;
  - `--feat`: a frozen wav2vec 2.0 Base (`sslfeat.py`: written out,
    only the weights downloaded, torchaudio's copy of the fairseq
    release, MIT; LibriSpeech 960 h, CC BY 4.0; WavLM Base+ passed over
    for its GigaSpeech hours, whose access terms weren't confirmed).
    The L1 distance of the conv features and the first 4 layers, each
    over the target's mean magnitude, between the output and the target
    magnitude, both with the noisy phase; 16 examples a batch, 2-s
    crops, bf16 (GPU memory: 10.6 GB peak on 12).

  Each 20k steps from gen4a, aligned pairs:

  | Gen, 1 step | Whisper WER | sub/del/ins | Parakeet WER | STOI | ESTOI | speech vs mask (dB) | balance (dB) |
  |---|---|---|---|---|---|---|---|
  | clean source | | | | | | | +3.2 / 0 / −5.1 / −9.1 |
  | mask (NN) | 52.5 % | 30/17/5 | 38.9 % | 0.552 | 0.351 | 0 | +3.1 / 0 / −4.0 / −6.6 |
  | gen4a | 60.9 % | 33/23/5 | 48.9 % | 0.600 | 0.364 | −8 / −14 / −18 / −19 | +5.9 / 0 / −7.7 / −14.1 |
  | gen5d (direct, band) | 62.4 % | 35/22/6 | 49.4 % | 0.603 | **0.386** | −9 / −14 / −16 / −20 | +5.4 / 0 / −5.5 / −12.1 |
  | gen5f (+ feat × 1) | 61.7 % | 34/22/6 | 47.7 % | 0.602 | 0.384 | −7 / −14 / −17 / −19 | +5.3 / 0 / −5.4 / −12.1 |
  | **gen5f3** (+ feat × 3) | **60.7 %** | 33/23/6 | **47.7 %** | 0.600 | 0.375 | −7 / −14 / −17 / −19 | +5.5 / 0 / −5.6 / −12.6 |

  The direct form alone gives the best ESTOI yet and the level within
  1 dB of the clean speech on the synthetic sets, the 1-2 kHz band
  2 dB closer to the clean balance; words don't follow. The feature
  loss buys words back, more with more weight (×1 → ×3: Whisper −1.0,
  Parakeet ±0). gen5f3 is in `crates/nr/model` (server c7ae809) and on
  the unit; still 8 points behind the mask on Whisper.
* **Sixth round (2026-11-28): more feature weight, and from scratch.**
  gen6f10: 20k steps from gen5f3 with the feature loss × 10; gen6s: 40k
  steps from nothing (direct, band, feature × 3, lr 1e-3).

  | Gen, 1 step | Whisper WER | sub/del/ins | Parakeet WER | STOI | ESTOI | speech vs mask (dB) | balance (dB) |
  |---|---|---|---|---|---|---|---|
  | gen5f3 | 60.7 % | 33/23/6 | 47.7 % | 0.600 | 0.375 | −7 / −14 / −17 / −19 | +5.5 / 0 / −5.6 / −12.6 |
  | gen6f10 (feat × 10) | 61.0 % | 32/24/5 | 48.0 % | 0.598 | 0.376 | −7 / −14 / −17 / −18 | +5.4 / 0 / −5.5 / −12.3 |
  | gen6s (from scratch) | 60.6 % | 31/25/5 | 47.0 % | 0.583 | 0.351 | −8 / −14 / −16 / −18 | +4.8 / 0 / −6.8 / −13.2 |

  Neither is a step: × 10 changes nothing measurable (the feature
  term's gain is spent by × 3), and from scratch trades ESTOI (back to
  the mask's 0.351) and more deletions for under a point of Parakeet.
  gen5f3 stays on the unit. The plateau is ~60.7 % Whisper against the
  mask's 52.5 %: more loss weighting won't close it; the next lever is
  the input (step 6, training jointly with the mask, or conditioning on
  more than the mask's output).
* **Seventh round (2026-11-28): step 6, the mask learning with Gen.**
  `gen.py --joint J`: the mask's weights train too, at J × the
  restorer's learning rate, no weight decay; its tuned copy is saved as
  `mask.pt` and scored as the nn0 column. gen7j: 20k steps from gen5f3,
  J = 0.1 (band, feature × 3).

  | 1 step | Whisper WER | sub/del/ins | Parakeet WER | STOI | ESTOI |
  |---|---|---|---|---|---|
  | mask (run7) alone | 52.5 % | | 38.9 % | 0.552 | 0.351 |
  | gen5f3 | 60.7 % | 33/23/6 | 47.7 % | 0.600 | 0.375 |
  | gen7j's tuned mask alone | 54.0 % | 30/18/5 | 41.1 % | 0.573 | 0.353 |
  | gen7j | 61.9 % | 35/22/5 | 49.8 % | 0.599 | 0.375 |

  Worse on both counts: Gen loses 1-2 points of WER at the same ESTOI,
  and the mask drifts towards what the restorer likes (STOI up, its own
  WER 1.5-2 points worse). The restorer was not starved of anything the
  mask throws away; joint training just spends the mask's capacity on
  the restorer's loss. J = 0.3 did not run (the GPU was taken by another
  program); with 0.1 already worse it was not repeated. gen5f3 stays.

### 5.5 Label every signal (2026-11-26, first version)

Plan (2026-11-23): instead of asking about one signal, mark every signal
in the waterfall's view with its class. Later, if worth it: a detector
on the waterfall image itself (boxes + classes in one pass).

First version, in the browser: **Signal labels** in the header's Panels
list (when the server has bins). The identifier over channelizer bins,
hopped across the view:

* **Source**: the wide IQ stream of §6 (one per session, 8-bit, ~1
  Mbit/s): a bin, 60 kS/s, clean within ±20 kHz of its centre. The page
  asks for the bin nearest a frequency it picks; while CW or RTTY Wide or
  an IQ recording holds the stream, only that bin (around the dial) is
  labelled.
* **A look** (`crates/sigid/src/skim.rs`, `sig_skim_find` and
  `sig_skim_id` in `sigid.wasm`, a second sigid worker): the last 4.096
  s of the bin; the signals in ±20 kHz found as the passband scanner
  finds them (the same 23.4 Hz bins, the same 4 dB extent and 10 dB peak
  over the floor, gaps of 190 Hz joined; `scan::regions_in`, shared),
  then up to 8 of them identified, new ones first (strongest first),
  then the longest ago, each from one FFT of the whole window (the
  identifier's 6 kHz around the signal: `scan::extract`, shared with
  the wide look). Natively 35 ms to find and ~90 ms a signal; in
  `sigid.wasm` (Node) 35 ms and ~370 ms.
* **Hops** (`ui/src/sigskim.ts`, pure and tested): bins every 30 kHz over
  the view; the next is the one visited longest ago (never visited
  first), nearer the dial on a tie; a view one bin wide stays, with a new
  look every 2 s. Only in views 1 MHz wide or narrower (`SKIM_MAX_SPAN`:
  ~34 bins, a few minutes a sweep). Leaving the view, the next hop goes
  into the new one at once.
* **Labels kept by RF** across looks: a signal found again (overlap over
  half the narrower one's width) keeps its label; its class
  probabilities are averaged half and half with each new naming; one not
  found in 2 looks of its range goes; 600 kept at most, 10 minutes.
* **Drawn** on the spectrum in two rows under the frequency labels: the
  class name over a line along the signal's extent, coloured by kind
  (amateur data blue, CW yellow, voice green, FSK orange, utility and
  military data purple, pictures pink, carriers, interference and radars
  grey); under 60 % dimmed with "?" (the second guess in the tooltip);
  under 30 % or noise not drawn. A click opens its decoder (or tunes to
  a voice or AM), else "What is this?". The 8 nearest the dial are also
  listed in the near-the-dial list (keyboard, screen readers, tests).

On air (the unit, 2026-11-26 around 00:40 UTC, 40 m):

* 120 kHz view (4 bins): within 90 s CW at 7009.5, 7010.7 and 7026.3
  kHz, the FT8 crowd at 7074-7077 kHz (74 % on the first look, then
  95-99 %), PACTOR at 7051.5 kHz; "Hell?" 45 % on a weak signal at 7007.
* 480 kHz view (16 bins): 6948-7240 kHz covered in 4 minutes: AM
  broadcasters at 7225, 7230 and 7240 kHz (66-98 %), an OTH radar around
  6980 kHz (97 %), CW and FT8 as above.
* Offline (`cargo run -p web888-sigid --example skimcs8`: 60 kS/s bins cut
  from the 4 MS/s recordings of 2026-11-15): the same regions and classes
  as the Python scan of that recording (`/data/iq/sigid-wide`).

Left:

* Several AM broadcasters side by side join into one region and are
  named at its middle (7245-7255 kHz: "SSB voice"); a broadcast's weak
  sidebands can split from its carrier. Regions wider than ~6 kHz need
  their own rule (split at carriers, or the measured AM mirror).
* A weak carrier's name changes between looks (AM, radar, interference,
  WSPR at 7210 kHz): the half-and-half average forgets fast; a slower
  average, or the figures of `measure.rs` (as "What is this?" uses them),
  for steady labels.
* FT8/FT4 slots: a look in the gap between slots sees little; averaging
  over looks covers it.
* Not on the phone yet (its More sheet has no switch).

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
  (V8, `simd128`): 36 % and 46 % of a core, in the worker (42 % and 58 %
  since the pile-up changes of §5.2). Getting there:
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

## 7. IQ recording from the page (2026-11-16)

A signal heard on the receiver, saved as IQ to study offline as heard
there (before: the workstation's HackRF on another antenna, or audio).

* **What**: the channelizer bin around the dial, through the wide IQ
  stream of §6: 60 kS/s complex, 60 kHz (clean within ±21 kHz of the
  centre). Needs the server's bins (the hello's `wide`).
* **Server**: `{"type": "wide", "on": true, "hz": dial, "bits": 16}` asks
  for 16-bit frames (`FLAG_I16`, still scaled per block by its peak;
  ~2 Mbit/s instead of ~1). Nothing is stored on the unit.
* **Page** (`ui/src/iqrec.ts`, the Rec IQ button in the header): 10 s to
  5 min; while recording the stream is 16-bit and keeps its bin (the
  wide skimmers share it and don't follow the dial meanwhile); samples
  lost on the way (frame numbers skip) become zeros, so file time stays
  air time, and the count is shown. Saved as `.cs16` (interleaved i16,
  no header) or `.wav` (stereo 16-bit, I left, Q right), scaled to the
  recording's peak at −1 dBFS, named by the start (UTC), centre and rate:
  `2026-11-16_0801_7072k_60k_web888.cs16`. Held as f32 in the page
  (480 kB/s; 144 MB at 5 min).
* **Test** (`ui/tools/cdp/iqrec.mjs`; the iq backend in bins mode
  replaying the 40 m recording at 1.92 MS/s): 10 s at 7074 kHz saved the
  bin 7042-7102 kHz, 600 000 samples, none lost; its spectrogram matches
  the source mixed to the same centre (correlation 0.95, no frequency
  shift, not inverted) and so do the samples (1 s of it correlates 0.999
  with the source decimated to 60 kS/s). The WAV reads as stereo 16-bit
  at 60 kHz. On the unit (2026-11-16): 10 s of 7050-7110 kHz, no samples
  lost, the FT8 segment 32 dB over the floor.
* **Not done**: on phones (no button there); replaying a file in
  `web888d --backend iq` needs resampling to 480 kS/s or more; the
  receiver's own channel (12 kS/s IQ around the dial) as an alternative to
  the bin on servers without one.
