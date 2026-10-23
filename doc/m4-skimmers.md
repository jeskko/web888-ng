# M4 skimmers: implementation plan

Status: **in progress** (2026-10-23; step 3). Decisions so far are in §9. Scope:
`design.md` §11 M4 and `feature-gap.md` §3 S: a decoder supervisor, FT8/FT4,
WSPR and FST4W autoruns, and spot reporting.
Done when (design.md): spots uploaded.

Concretely, done when:

* Autoruns from the config decode FT8/FT4 every slot and WSPR every
  2 minutes on the bands configured, on Skimmer leases. They yield to
  users when the channels run out and resume afterwards (stock bug #55).
* Spots go to PSKReporter, WSPRnet and MQTT, and appear in the UI and on
  the admin page.
* On the same audio, the FT8 decoder finds at least 90 % of what the
  WSJT-X `jt9` finds (target; §3.3 has the measurement), with no more
  than 1 % false decodes.
* Only spots from a **live** source are uploaded. The iq backend on a
  recording, the sim backend and WAV corpora never upload (§6.3).

---

## 1. Picture

```
 hw thread ─RX blocks─▶ DSP pool: SkimJob per band (Skimmer lease)
                          │ channel IQ 48 kS/s → USB audio 12 kS/s
                          │ slot buffer aligned to UTC (frame clock)
                          ▼
                    decoder thread(s), low priority, deadline per slot
                     ftx: FT8 / FT4 / FST4W     wspr: WSPR-2
                          │ Spot
                          ▼
                     spot bus (tokio broadcast)
          ┌───────────┬──────────┬──────────┬───────────┐
       PSKReporter  WSPRnet    MQTT     /api/spots   admin page
       (IPFIX/UDP)  (HTTP)              + WS to UI
```

## 2. Decoders, in process

Both decoders are Rust crates with no C dependencies. They run in their
own threads, so a slow decode can't stall the DSP pool, and Rust safety
covers what the child-process design (`design.md` §9) was for. Child
processes stay the plan for HFDL and DRM (milestone D).

### 2.1 `crates/ftx`: FT8, FT4, FST4W

* Base: a Rust port of ft8_lib (MIT, Kārlis Goba; RaspSDR's fork with OSD,
  subtraction and FST4/FST4W, `upstream/server/extensions/FT8/ft8_lib`).
  MIT notices are kept in the ported files.
* Shared parts: CRC-14, LDPC (174,91) belief propagation and OSD, 77-bit
  message pack/unpack with the callsign hash table, the encoder (for
  tests and subtraction).
* The demodulator front end is where the stock decoder loses (about 75 %
  of WSJT-X's decodes, from the fork's own README). Improvements follow
  the published algorithm (Franke, Somerville, Taylor, "The FT4 and FT8
  Communication Protocols", QEX 2020): per-candidate downsampling and
  fine sync, soft metrics over 1-3 symbols, time-domain subtraction, and
  multiple passes. Each step is measured against `jt9` (§3.3).
* **Licence note.** WSJT-X's Fortran is GPLv3 without "or later", and
  this project is GPL-3.0-or-later. So the FT8/FT4 code is written from
  ft8_lib and the paper; WSJT-X is only used to compare behaviour, not
  translated. wsprd's C is GPL-3.0-or-later and is ported directly (§2.2).

### 2.2 `crates/wspr`: WSPR-2

* A port of WSJT-X's `lib/wsprd` (C, GPL-3.0-or-later; Franke/Taylor,
  Karn's Fano decoder, Jenkins' nhash). rustfft replaces FFTW.
* Checked against the local `wsprd` on the same WAVs.
* FST4W-120 (and longer periods) come from `ftx`, on the same 2-minute
  schedule.

### 2.3 Load

The stock server runs 11 FT8 channels on the Zynq's two cores with
ft8_lib. Our decoder does more work per candidate, so it gets a budget:

* a per-slot deadline (FT8 decodes after 13 s of signal; results must be
  in before the next slot ends), and passes that stop at the deadline
* decoder threads at `SCHED_IDLE` / nice 19, at most `cores - 1`
* CPU per decoder and per band on the admin page and in `/metrics`

The board numbers come when the test unit is here; benches run on the
production unit's A9 meanwhile (nice 19, as in M3; the stock server keeps
it busy, so these are upper bounds).

* 2026-10-23, first decoder: ~20 s of CPU per busy 40 m slot (desktop
  0.56 s). After the work in §3.3: **4.9 s** (desktop 0.14 s, ~35x).
  NEON auto-vectorisation (`+neon`) changes nothing.
* Cutting candidates or passes costs sensitivity quickly (passes 3 -> 2:
  -1.8 points for -18 % time), so instead each band gets a time budget
  per slot (`Decoder::decode_until`): strongest candidates first, later
  passes only if time is left. On the desktop 0.1 s keeps 88.5 % of
  jt9, 0.2 s the full 91.3 % (A9: roughly 3.5 s and 7 s). The skimmer
  divides the idle CPU between the bands.

## 3. Test corpus and scoring

### 3.1 Recordings

HackRF on the HF antenna (kept outside the repo). The first one is
`2026-10-23_8600k_4000k_40m30m_12min.cs8` (4 MS/s around 8.6 MHz, 12 min,
evening): FT8 and FT4 on 40 m and 30 m, plus WSPR. Its start time
(UTC, from the host clock) is in a `.start` file next to it.

### 3.2 Tool

`ftx` example `slotcut`: cuts USB audio at a dial frequency from an IQ
recording into 12 kHz WAVs, one per UTC slot, named like WSJT-X does
(`YYMMDD_HHMMSS.wav`). The same code path (channel → USB 12 k → slot)
is used by the skimmer job, so the corpus tests the production chain.

### 3.3 Score

`jt9 -8` (and `-5` for FT4, `wsprd` for WSPR) on each WAV is the
reference. A test compares the message sets:

* found = in both
* missed = only in jt9
* extra = only ours. Extras are listed, and checked by hand at first:
  they may be real decodes that jt9 missed, or false decodes.

The scores go in this doc at each improvement step.

| date | change | 40 m (1600 jt9 decodes) | 30 m (1267) | desktop s/slot |
|---|---|---|---|---|
| 2026-10-23 | first decoder (30 slots of 40 m, tuning set) | 77.7 % | – | 0.56 |
| 2026-10-23 | subtracted positions retried, LLR scale 5.5, sub-sample timing, OSD order 2 (60) with a plausibility filter | 86.8 %, 30 extra | 90.1 %, 12 extra | 0.7 |
| 2026-10-23 | OSD by CRC syndromes (orders 2 and 3 over all 91 bits), acceptance by distance ratio to the runner-up; subtraction in the frequency domain | 91.3 %, 54 extra | 92.3 %, 17 extra | 0.25 |

OSD at that depth meets about 8 chance CRC-valid codewords per call. A
real decode is much closer to the received bits than the runner-up (median
ratio 0.54); a false one is just the best of the chance ones (median
0.93). The ratio limit is 0.65, or 0.75 when a callsign in the message has
been heard before.

The extras are now almost all real traffic (QSO partners seen in other
slots); jt9's AP decodes (28 and 25 of the misses) need a priori
information we don't use yet. 30 m wasn't used for tuning. A small labelled
corpus (a few WAVs and the expected messages) goes into the repo for CI.
The rest stays in /data.

## 4. Skimmer runtime (`crates/skim`)

* Config `[[skimmer]]` entries: `mode` (ft8, ft4, wspr, fst4w), `dial`
  (Hz), optional `hours` (UTC ranges, e.g. `"18-06"`), `enabled`. Band
  presets name the usual dials (`ft8 = ["40m", "30m", "20m"]`).
* A band = one Skimmer lease + one `SkimJob` on the DSP pool. The job
  mixes the channel down to USB audio at 12 kS/s and fills slot buffers.
  Slot times come from the frame clock (PPS-disciplined on the board,
  wall clock on the iq backend).
* One RX channel (48 kS/s, ±24 kHz) can carry several modes at nearby
  dials, e.g. FT8 7.074 + WSPR 7.0386 is too far, but FT4 10.140 + FT8
  10.136 fits. The supervisor packs dials onto channels when they fit.
* **Yielding.** Skimmer leases yield to users. When a web or protocol
  request finds no free channel, it takes the newest skimmer channel
  (regardless of `preempt`), and the skimmer waits again. It gets a
  channel back when one is free. The class order for everything else
  is unchanged. A per-skimmer `keep = true` marks one that never yields.
* A restart counter, a last-decode time and decodes per slot for each
  band; a band that hasn't produced a slot in 5 minutes is restarted and
  logged.

## 5. Spots

* `Spot { time, mode, dial_hz, freq_hz, snr_db, dt_s, drift (WSPR),
  message, call, grid, power (WSPR), band }` on a tokio broadcast bus.
* `/api/spots?since=&mode=&band=` (recent spots, ring of ~10k in RAM),
  and protocol 2 pushes new spots to UIs that ask.
* UI: a spot list panel (filter by band and mode, click to tune), and
  markers on the scale for the visible band. The map comes later
  (milestone M).
* Admin: per-band status (lease, last decode, decodes per slot, CPU),
  uploader state (queued, sent, last error).

## 6. Reporting

### 6.1 PSKReporter

* IPFIX over UDP to `report.pskreporter.info:4739`. Receiver record
  (callsign, locator, decoder software "web888-ng x.y"), and sender
  records (call, frequency, SNR, mode, locator when known, time).
* Batches every 5 minutes, with a random offset. A call+band+mode pair
  is sent at most once per 5 minutes. This follows their developer page.
* Tested against a local UDP listener that parses our IPFIX.

### 6.2 WSPRnet

* HTTP POST of each decode (the wsprd/Kiwi form fields), with a retry
  queue.

### 6.3 Live only

Each backend says whether it is live: `zynqsdr` and the iq backend on
SoapySDR are; the iq backend on a file, the sim and WAV inputs aren't.
The uploaders drop spots from a non-live source, and the reason shows on
the admin page. A config switch can't override this. Uploads also need a
callsign and a locator in `[station]`, and `upload = true` per service.
The development station uploads with a callsign of its own, set in its
local config (never a default in the repo).

### 6.4 MQTT

`[mqtt]` broker URL. Spots go to `<prefix>/spots/<mode>` as JSON, and
skimmer start/stop events to `<prefix>/skimmer`. Later: temperature,
users.

### 6.5 Station position

`[station] locator`, or gpsd (`[station] gpsd = "127.0.0.1:2947"`),
which takes a 6-character locator from the fix.

## 7. Steps (each ends in a commit)

| # | content | done when |
|---|---|---|
| 1 | `ftx` common: CRC, LDPC BP + OSD, message pack/unpack + hash table, encoder | round-trip tests on WSJT-X message examples; encoder matches `ft8code` tones |
| 2 | FT8 decoder (ft8_lib front end), `slotcut` tool, scoring against jt9 | baseline score on the corpus in §3.3 |
| 3 | FT8 front-end improvements, one at a time, with scores | ≥ 90 % of jt9, ≤ 1 % false |
| 4 | FT4 | scored on the corpus |
| 5 | skimmer runtime: config, SkimJob, slot clock, decoder threads, yielding leases | sim + iq-file tests: decodes every slot; a web user takes the last channel and the skimmer resumes after |
| 6 | spot bus, `/api/spots`, protocol push, UI spot list + markers, admin status | browser check (muted) |
| 7 | PSKReporter uploader, live-only gate | fake-server test; then live from the HackRF |
| 8 | WSPR port, WSPRnet uploader; FST4W | WSPR scored against wsprd; live upload |
| 9 | MQTT, gpsd, A9 benches, docs, release (with go-ahead) | benches on the production unit (nice 19) |

### Progress

* Steps 1-3 (FT8) done 2026-10-23; FT4 (4) postponed until after the
  runtime, which got FT8 end to end first.
* Step 5, first part (2026-10-23): crate `web888-skim`. `[[skimmer]]`
  bands with UTC hours, a `BandJob` per band on the DSP pool (channel
  tuned 3 kHz above the dial, 48 -> 12 kS/s USB audio), a frame clock
  (min-delay host time; median DT 0.2 s live), decoder threads at
  `SCHED_IDLE` sharing the time to the next slot, the spot bus. Leases:
  `[limits] skimmers_yield` (default on). Live from the HackRF: ~45-135 ms
  of desktop CPU per busy 40 m slot.
* Step 7, first part: PSKReporter uploader (5-minute batches, once an
  hour per call and band, live spots only), `[station]`, `[report]`.
  The developer page's layout (scope field in the receiver template, all
  records in one set) was silently dropped; the stock Web-888 layout,
  byte for byte (tested against a harness of the stock code and a capture
  from a stock unit), works: live reports as the development receiver
  showed on PSKReporter on 2026-10-23 21:11 UTC. Their test listener can't
  check anything (the bare domain is behind a web proxy, and nothing sent
  to port 14739 shows on the analysis page).

* Step 6 (2026-10-24): the spot bus keeps 10k spots and hands a
  session the recent ones and the new ones without loss or doubles.
  Protocol: `hello.spots` says the server runs skimmers. The client
  sends `{"type":"spots","on":true}`. The server answers with the last 30
  minutes (at most 1000), then sends each new batch once a second.
  `/api/spots?since=&mode=&band=` sits behind the same gate as `/ws`.
  `/api/admin/skimmers` lists the bands (state, spots and CPU per slot,
  last slot) and the PSKReporter uploader. Metrics: `skim_*`,
  `pskreporter_reports_total`.
  UI: a spot list beside the waterfall (below it on narrow screens),
  filtered by band, mode and text. Clicking a row tunes to the skimmer's
  dial in USB. The scale shows the last 40 s of spots as call labels,
  up to 3 rows. The admin status page has a skimmer card. Checked live
  from the HackRF with `ui/tools/cdp/spots.mjs`.

* Step 4, FT4 (2026-10-24): `ft4.rs` follows FT8's decoder, with FT4's
  4-GFSK at 20.8 Bd and BT 1.0, its four 4×4 Costas arrays and the
  message scrambling. Per-candidate work runs at 666.7 S/s (32 samples
  per symbol). Soft bits come from 1, 2 and 4 symbols. Subtraction uses
  a least-squares amplitude, because the full-symbol ramps take the
  reference to zero. The synthesizer's `Shape::Ft4` adds those ramp
  symbols. Corpus: 40 m at 7047.5 kHz, 96 slots, jt9 `-5 -d 3` as the
  reference (`ft4_7047.5k/jt9.txt`; jt9 can't read the `.5` in the slot
  names, so it runs per file). Score: **89.3 %** (268 of 300). The 11
  extras look like real traffic (QSOs of calls in neighbouring slots).
  DT and frequency agree with jt9's, and SNR is within 1 dB.
  2.7 s of desktop CPU for the 96 slots. The skimmer runs `mode = "ft4"`
  bands in 7.5 s slots (6.6 s collected); it decoded live from the
  HackRF.

* Step 8, WSPR decoder (2026-10-24): crate `web888-wspr`, a port of
  wsprd (`wsprd.c`, the utilities, Karn's Fano decoder, nhash). Its
  defaults are wsprd's, without OSD (WSJT-X Fortran, GPLv3 only, and off
  in wsprd by default). One change: a decode is rebuilt for subtraction
  from its bits, not from the re-packed text, so an unknown hashed call
  no longer subtracts the wrong signal. Test vectors come from
  `wsprsim -cd` and `nhash`, built from the upstream sources in a
  scratch dir. Corpus: 5 slots each on 40 m and 30 m (`wspr_*/wsprd.txt`
  from `wsprd -f`). All **64 of wsprd's 64** decodes, the same SNR, DT,
  frequency and drift (two SNRs 1 dB apart), and one extra (HB9T JN36
  23). Like wsprd, 91 % of the time goes to Fano on candidates that
  fail. The default `maxcycles` is 3000 (wsprd: 10000), which keeps 64
  of 65 for less than half the time. A9 at nice 19 on the loaded unit:
  73 s for a busy 40 m slot (46 s at 1000 cycles, same 12 decodes).
  `decode_until` takes a deadline.

## 8. Open

* Board CPU: how many FT8 bands fit next to 12 users. Measured on the
  test unit. Until then the decoder budget is configurable.
* CW/RTTY skimmers (S2) come after M4 (feature-gap §5, question 8).

## 9. Decisions

1. **FT8/FT4 engine** (2026-10-23): a Rust port of ft8_lib, then improved
   step by step and scored against jt9. In process, low priority.
2. **WSPR**: a Rust port of wsprd. FST4W from the ft8_lib port.
3. **Uploads during development**: allowed for spots we're fairly
   confident of, with a callsign separate from the one another station
   of the developer's already uploads with, and **only from live
   signals, never recordings**.
