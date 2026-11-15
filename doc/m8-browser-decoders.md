# M8: more decoders in the browser (plan B)

Status: in progress (planned 2026-11-15). Done: item 1 (CW; `crates/cw`,
`crates/cw-wasm`, `ui/src/cw.ts`, `ui/src/ctl/cw.tsx`, §2.1), item 2
(FSK/RTTY; `crates/fsk`, `crates/fsk-wasm`, `ui/src/rtty.ts`,
`ui/src/ctl/rtty.tsx`, §2.2), items 3 and 4 (SSTV and HF fax;
`crates/img`, `crates/img-wasm`, `ui/src/img.ts`, `ui/src/ctl/img.tsx`,
§2.3-2.4); item 5 (SITOR-B decodes; NAVTEX messages listed per signal; not
tried on air), item 7 (the neural CW decoder, §5), item 9 (wide
skimming, §6) and item 10 (PSK31/63/125; `crates/psk`, `crates/psk-wasm`,
`ui/src/psk.ts`, `ui/src/ctl/psk.tsx`, §2.5).

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
| 8 | optional: **signal identifier** (done, §5.3) | trained on synthetic + labelled recordings (§5) | names the mode of a signal in the passband (RTTY 50/450, fax, SSTV, FT8, STANAG 4285...) and starts its decoder |
| 9 | **wide skimming**: a channelizer bin (60 kS/s IQ, ~40 kHz) to the browser for the CW and RTTY skimmers | — | a whole CW or RTTY sub-band skimmed at once in the page; the unit only passes the bin on |
| 10 | **PSK31/63/125** (BPSK, Varicode) | new (G3PLX's Varicode) | every BPSK signal in the passband found (carrier, rate) and decoded |
| 11 | **HFDL** (aircraft data link, 1800 Bd PSK; ARINC 635, dumphfdl GPL-3 as the reference) | new | squitters, logons, position reports and ACARS decoded, checked by their CRCs; aircraft positions and ground stations on the map (§2.6) |
| 12 | **JS8** (JS8Call: FT8's waveform, its own code and frames; Normal, Fast, Turbo, Slow) | the FT8 decoder, generalised | heartbeats, directed messages and free text, frames joined per frequency; senders with a grid on the map (§2.7) |
| 13 | later: ALE 2G, Olivia | new | — |

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

* **Classes** (15): noise, carrier, CW, 2-FSK (RTTY 45-75 Bd, SITOR-B /
  NAVTEX, ASCII, DWD 50/450: the RTTY panel works out which), PSK31/63/125,
  FT8, FT4, WSPR, ALE, STANAG 4285, MIL-STD-188-110A, SSTV (13 modes), HF
  fax, SSB voice, AM. A row's Decode opens the page's decoder (CW, RTTY,
  fax/SSTV, FT8/FT4/WSPR); Tune tunes to voice (LSB below 10 MHz) or AM.
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
voice at 20-40 % (top, unsure) or as CW. Next: on-air speech in training
(the SSB segments of the 2026-11-15 contest recordings).

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
