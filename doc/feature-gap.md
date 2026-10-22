# Feature gap: web888-ng vs the stock Web-888 server and KiwiSDR

Status: **proposal** (2026-10-22). Order and omissions are open (§4).

Compared:

* **Stock**: the Web-888 server (RaspSDR's KiwiSDR fork), source as of
  2026-09-27 (release web-888-20260919). From its source and release notes.
* **Kiwi**: official KiwiSDR v1.902 (2026-07), from its CHANGE_LOG.
* **OWRX+**: OpenWebRX+ (1.2.x), as a reference for a modern multi-user
  server. Listed only where it shows a direction.
* **ng**: web888-ng v0.2.0.

Legend: ✓ has it, ~ partly, ✗ no, – not applicable.

## 1. Where web888-ng already differs

* **Ahead of stock:**
  * HPSDR P1 for up to 7 receivers: stock has the code but doesn't build it.
  * Leases with a queue, listen-along, and a pre-emption policy by class.
  * Shared and time-shared waterfall views.
  * A DSP pool with load shedding.
  * Prometheus metrics, a config file with hot reload, an admin JSON API.
  * TLS on the same port.
  * An open driver and open bitstreams (separate repos).
  * A read-only image with a config partition.
* **Behind:** almost everything a listener sees beyond SSB/CW/AM. These
  are the tables below.

## 2. Gaps by area

### 2.1 Receiver and audio

| feature | stock | Kiwi | ng | plan |
|---|---|---|---|---|
| USB/LSB/CW/AM, passband presets | ✓ | ✓ | ✓ | – |
| SAM (+ SAU/SAL/SAS, PLL speed, fade leveler) | ✓ | ✓ | ✗ | **R** |
| NBFM/NNFM + de-emphasis, noise squelch | ✓ | ✓ | ✗ | **R** |
| IQ mode (stereo IQ to the browser / recorder) | ✓ | ✓ | ✗ | **R** |
| C-QUAM AM stereo | ✓ | ✓ | ✗ | omit |
| AGC controls (threshold, slope, decay, hang) | ✓ | ✓ | ~ F/M/S + gain | **R** (threshold, decay) |
| Noise blanker (2 algorithms) | ✓ | ✓ | ✓ (1) | – |
| NR (LMS, WDSP, spectral) | ✓ | ✓ | ✓ spectral | – |
| Auto-notch | ✓ | ✓ | ✗ | **R** |
| Squelch | ✓ | ✓ | ✓ | – |
| Wider audio (24/36 kHz) for AM/DRM broadcast | ✓ | ~ | ✗ (12k, ±5 kHz) | **R** (24k) |
| Audio compression (IMA ADPCM) | ✓ | ✓ | ✗ (PCM16) | **R** (ADPCM; Opus later) |
| RF attenuator from the UI | ✓ | ✓ (Kiwi-2) | ✗ (HPSDR only) | **R** |
| S-meter calibration, overload mute | ✓ | ✓ | ✗ | **R** |
| Frequency offset (converters, transverters) | ✓ | ✓ | ✗ | **R** |
| DRM (Dream + FDK-AAC) | ✓ | ✓ | ✗ | **D** (out of process; A9 CPU?) |

### 2.2 Waterfall and spectrum

| feature | stock | Kiwi | ng | plan |
|---|---|---|---|---|
| Zoom, pan, click-tune | ✓ | ✓ | ✓ | – |
| Waterfall per user / views | share or exclusive | ✓ 8 ch (v1.900) | ✓ shared views + time-sharing | – |
| Auto levels | ✓ | ✓ | ✓ | – |
| Colormaps (several + editor) | ✓ | ✓ | ✗ (1) | **R** (a few, no editor) |
| Spectrum peak hold, audio (AF) spectrum | ✓ | ✓ | ✗ | **R** |
| Timestamps on the waterfall, save as image | ✓ | ✓ | ✗ | **R** |
| Window/interpolation/averaging choices | ✓ | ✓ | fixed (Welch) | omit (ours averages) |
| Waterfall compression | ✓ ADPCM | ✓ | ✗ (u8 dB per bin) | later (measure first) |

### 2.3 Listener UI

| feature | stock | Kiwi | OWRX+ | ng | plan |
|---|---|---|---|---|---|
| Band bars + band plan (ITU regions) | ✓ | ✓ | ✓ | ✗ | **R** |
| DX labels: own, EiBi schedules, community | ✓ | ✓ | bookmarks | ✗ | **R** (own + EiBi) |
| Frequency memories, VFO A/B | ✓ | ✓ | ✓ | ✗ | **R** |
| Audio recording (WAV), squelch-triggered | ✓ | ✓ | MP3 | ✗ | **R** (client side) |
| Queue / camp on a user's channel | ✓ | ✓ | ✗ | ✓ (listen-along) | – |
| Users on the scale | list | list | ✗ | ✓ markers | – |
| Chat | ✗ | ✗ | ✓ | ✗ | omit |
| Mobile layout, keyboard shortcuts, URL state | ✓ | ✓ | ✓ | ~ | **R** (fill gaps) |
| Themes | ✓ | ~ | ✓ | ✗ (dark) | later |
| Map (receivers, spots, aircraft) | ✗ | TDoA map | ✓ | ✗ | with **S** (spots) |
| Languages | ✗ | ✗ | ✗ | ✗ | omit |

### 2.4 Decoders

| decoder | stock | Kiwi | OWRX+ | ng | plan |
|---|---|---|---|---|---|
| FT8/FT4/FST4W autorun → PSKReporter, MQTT | ✓ | ✓ | ✓ | ✗ | **S** |
| WSPR autorun → WSPRnet, MQTT | ✓ | ✓ | ✓ | ✗ | **S** |
| Autorun pre-empted by users, resumes after | ~ (stock #55: doesn't resume) | ✓ | – | lease class exists | **S** |
| CW decoder (single) | ✓ | ✓ | ✓ | ✗ | **B** |
| CW skimmer (multi-signal; RBN?) | ✓ | ✓ exp. | ✗ | ✗ | later |
| FSK/RTTY, NAVTEX, DSC, SITOR | ✓ | ✓ | ✓ | ✗ | **B** |
| HF FAX, SSTV | ✓ | ✓ | ✓ background | ✗ | **B** |
| Time codes (DCF77, MSF, WWVB, …) | ✓ | ✓ | ✗ | ✗ | **B** |
| HFDL | ✓ (opt.) | ✓ | ✓ | ✗ | **D** (dumphfdl process) |
| ALE 2G | ✓ | ✓ | ✗ | ✗ | later |
| AIS, ACARS (VHF airband mode) | ✓ | ✗ | ✓ | ✗ | with **V** |
| MFSK (digi_modes), STANAG-4285, Loran-C, IBP scan | ✓ | ✓ | ~ | ✗ | omit (IBP: maybe as a UI tool) |
| S-meter graph, IQ display, space weather | ✓ | ✓ | ✗ | ✗ | later (small, client side) |

### 2.5 Clients, APIs and network

| feature | stock | Kiwi | ng | plan |
|---|---|---|---|---|
| Kiwi WebSocket protocol (kiwirecorder, wsprdaemon, SuperSDR) | ✓ | ✓ | ✗ | **K** |
| `/status`-style listing data, `/snr`, `/users` | ✓ | ✓ | `/api/status`, metrics | **K** |
| HPSDR P1 | ✗ (not built) | ✗ | ✓ | – |
| IQ stream / rtl_tcp / SpyServer | ✗ (dormant) | ✗ | ✗ | later (design §7) |
| MQTT (spots, start/stop, temperature) | ✓ | ✗ | ~ | **S** |
| Public listing (rx-888.com / rx.kiwisdr.com) | ✓ rx-888.com | ✓ | ✗ | open question |
| Vendor reverse proxy (frpc), DUC/no-ip, UPnP | ✓ | ✓ | ✗ | omit (document a reverse proxy) |
| ACME certificates | ✓ | ✗ | ✗ (self-signed/files) | later |
| TDoA (GPS-timestamped IQ, Kiwi TDoA network) | ✗ (disabled) | ✓ | ✗ | after **K** + open FPGA timestamps |

### 2.6 Admin and operations

| feature | stock | Kiwi | ng | plan |
|---|---|---|---|---|
| Status, users + kick, config, passwords | ✓ | ✓ | ✓ | – |
| Session time limit, inactivity limit | ✓ | ✓ | **config keys exist but aren't enforced** | **fix now** |
| Per-IP daily limit, IP block list, ban from the user list | ✓ | ✓ | per-IP sessions only | **O** |
| Channels without a password (open + private sets) | ✓ | ✓ | ✗ (all or nothing) | **O** |
| Receiver info page (name, location, antenna, photo) | ✓ | ✓ | ✗ | **O** |
| Logs in the admin page | ✓ | ✓ | ✗ (journald) | **O** |
| Updates (channels, rollback) | ✓ zip | ✓ | ✗ (reflash) | **O** (os: A/B or package feed) |
| Network config (static IP, Wi-Fi) | ✓ | ✓ | ✗ (os) | **O** (os) |
| Usage analytics (heatmaps, visitors) | ✓ | ~ | metrics | omit (Prometheus/Grafana) |
| Browser shell console | ✓ | ✓ | ✗ | omit (ssh) |
| Backup / restore | ✓ SD | ✓ | config file only | **O** (download/upload config) |

### 2.7 Hardware modes (with the hw repo)

| feature | stock | ng | plan |
|---|---|---|---|
| HF 0–62 MHz at 122.88 MHz | ✓ | ✓ | – |
| 32 MHz "narrowband" (61.44 MHz ADC) | ✓ | ✗ | omit unless asked |
| VHF airband by undersampling (98.304 / 110.592 MHz clocks, RF switch) | ✓ | ✗ (Si5351 plan exists, test unit only) | **V** |
| External 10 MHz reference, GPSDO output on CLK2 | ✓ | ✗ | **V** |
| ADC dither, PGA, DC-offset trim | ✓ | ✗ | with **V** (bitstream/driver) |
| GPS: position via gpsd, PPS clock correction | ✓ | PPS loop ✓, gpsd ✗ | **S** (grid for spots) |

## 3. Proposed order

Each step is a milestone like M3 (plan, steps, tests).

1. **Fix now** (small, before M4): enforce `session_minutes` and
   `idle_minutes`.
2. **S — skimmers (M4)**:
   * A decoder supervisor (child processes, restart, CPU accounting).
   * FT8/FT4/FST4W and WSPR autoruns on Skimmer-class leases, pre-empted
     by users and resumed after (stock's open bug).
   * Uploads to PSKReporter, WSPRnet and MQTT.
   * Spots in the admin page, and as markers or a list in the UI.
   * gpsd for the grid.

   This is what keeps a unit useful when nobody is listening, and what
   the stock firmware is mostly run for. It's the reason to switch.
3. **R — receiver parity (M5)**:
   * DSP: SAM, NBFM, IQ mode, auto-notch, AGC threshold/decay.
   * Audio: a 24 kHz option, ADPCM audio.
   * RF attenuator, frequency offset, S-meter calibration.
   * UI: band bars and plan, DX labels (own + EiBi), memories, WAV
     recording, a few colormaps, peak hold, waterfall timestamps.
4. **K — Kiwi protocol (M6)**: kiwirecorder, wsprdaemon and SuperSDR
   work. `/status` and `/snr` for listings. It opens the existing tool
   ecosystem without us writing clients.
5. **B — interactive decoders in the browser (M7)**: CW, FSK/RTTY,
   NAVTEX/DSC, FAX, SSTV, time codes, as WASM/JS fed from the audio the
   page already gets.
   * The stock server runs these on the A9 per user. Our bench (m3 §4)
     says the A9 has little to spare, while a browser has plenty.
   * Only autoruns (S) and things needing more than the audio (D) run on
     the unit.
6. **O — operations (with the os repo, can run alongside)**:
   * Access: per-IP daily limit, block list, open + private channel sets.
   * Receiver info page, log view, config download/upload.
   * os work: updates, network config.
7. **D — heavier server-side decoders**: DRM (FDK-AAC strictly out of
   process; measure A9 CPU first) and HFDL (dumphfdl).
8. **V — VHF airband and clocking (with hw)**: airband mode, AIS/ACARS,
   external reference, ADC controls. These need the test unit and
   bitstream work.
9. **Later**: CW skimmer, ALE, themes, IQ stream/rtl_tcp/SpyServer, ACME,
   TDoA (needs K and open-FPGA timestamps), waterfall compression.

**Proposed omissions:**
* chat, languages
* C-QUAM, MFSK/STANAG-4285/Loran-C
* waterfall window and interpolation choices (ours averages)
* the vendor reverse proxy, DUC and UPnP (document a reverse proxy
  instead)
* the browser console (ssh instead)
* usage analytics (metrics instead)
* 32 MHz narrowband mode

## 4. Open questions

1. Is S (skimmers) first? It is in the original plan (design §11, M4)
   and the most-used stock feature.
2. R before K? K first gives automation users (kiwirecorder, wsprdaemon)
   their tools sooner. R first makes the page itself a full receiver.
3. Interactive decoders in the browser (B) rather than on the unit?
4. Public listing: register at rx-888.com like stock (their API), at
   rx.kiwisdr.com (needs K), or nothing for now?
5. VHF airband (V): wanted, and how soon? It depends on the test unit and
   hw work.
6. Anything in the omit list that you use?
