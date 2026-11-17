# Feature gap: web888-ng vs the stock Web-888 server and KiwiSDR

Status: **proposal** (2026-10-22), **revisited 2026-11-12** for web888-ng
v0.10.0, brought up to v0.12.0, and on 2026-11-17 to main after v0.14.0.
Done since: S (skimmers, M4), K (the Kiwi protocol, M5), the channelizer
with its zoom engine (M6), VHF (experimental), the os side of O with
metrics and OS updates from the admin page, logs and backups, a first
map (M), B (the browser decoders, M8), and most of R (the listener tools
of 2026-11-17: `doc/m2-web-ui.md` §11). The ng column shows main; the
stock, Kiwi and OWRX+ columns are still the 2026-10-22 survey (not
re-checked). Milestone letters here are this proposal's; `design.md` §11
has the numbered milestones. §3 is the remaining order, proposed again;
§5 the questions still open.

Compared:

* **Stock**: the Web-888 server (RaspSDR's KiwiSDR fork), source as of
  2026-09-27 (release web-888-20260919). From its source and release notes.
* **Kiwi**: official KiwiSDR v1.902 (2026-07), from its CHANGE_LOG.
* **OWRX+**: OpenWebRX+ 1.2.127 (2026-10), the active fork of OpenWebRX.
  Upstream OpenWebRX is dormant: its last release is 1.2.2 (2023), and
  develop was last touched in 2024. §4 covers both.
* **ng**: web888-ng v0.12.0 with web888-ng-hw v0.5.0 and web888-ng-os
  v0.9.0 (2026-11-12; the 2026-10-22 survey had v0.2.0).

Legend: ✓ has it, ~ partly, ✗ no, – not applicable.

## 1. Where web888-ng already differs

* **Ahead of stock:**
  * **The channelizer bitstream** (the os default since v0.7.0): the
    whole band in bins, 32 receivers, the full-band waterfall, and zoomed
    views computed in the FPGA (16 distinct 480 kHz views at 20 lines/s
    on the A9). Stock has 12 receivers and 2 waterfall channels shared
    by everyone.
  * HPSDR P1 for up to 7 receivers: stock has the code but doesn't build it.
  * Leases with a queue, listen-along, and a pre-emption policy by class;
    skimmers yield to listeners and resume.
  * A DSP pool with load shedding.
  * Prometheus metrics, a config file with hot reload, an admin JSON API.
  * TLS on the same port; an HTTP audio fallback.
  * An open driver and open bitstreams (separate repos).
  * The OS: A/B updates with rollback (RAUC), a config partition, tunnels
    (frp, cloudflared, rathole, chisel) from `web888.txt`.
  * JTTY decoding in the browser (no cost to the unit).
* **Behind:** many listener tools beyond the core receiver (§2.1-2.3),
  most interactive decoders (§2.4), access control and the receiver
  info page (§2.6).

## 2. Gaps by area

### 2.1 Receiver and audio

| feature | stock | Kiwi | ng | plan |
|---|---|---|---|---|
| USB/LSB/CW/AM, passband presets | ✓ | ✓ | ✓ | – |
| SAM (+ SAU/SAL/SAS, PLL speed, fade leveler) | ✓ | ✓ | ✓ SAM/SAL/SAR/SAS, one PLL speed, carrier AGC | – (PLL speed choice later) |
| NBFM/NNFM + de-emphasis, noise squelch | ✓ | ✓ | ✓ NBFM (v0.10.0: ±5 kHz at 12 kS/s, 300 Hz de-emphasis, squelch; Kiwi `nbfm`) | – |
| IQ mode (stereo IQ to the browser / recorder) | ✓ | ✓ | ✓ the web UI's IQ mode (PCM), Rec saves an I/Q WAV; Kiwi clients | – |
| C-QUAM AM stereo | ✓ | ✓ | ✗ | omit |
| AGC controls (threshold, slope, decay, hang) | ✓ | ✓ | ✓ F/M/S or custom hang and decay, max gain (threshold); no slope | – |
| Noise blanker (2 algorithms) | ✓ | ✓ | ✓ (1) | – |
| NR (LMS, WDSP, spectral) | ✓ | ✓ | ✓ spectral | – |
| Auto-notch | ✓ | ✓ | ✓ spectral, any number of carriers (web UI) | – |
| Squelch | ✓ | ✓ | ✓ | – |
| Wider audio (24/36 kHz) for AM/DRM broadcast | ✓ | ~ | ✓ 12k (±5 kHz) default, 24k (±10 kHz) per session in the web UI; Kiwi 12k | – |
| Audio compression (IMA ADPCM) | ✓ | ✓ | ✓ the web UI: Opus 24-48 kbit/s (μ-law while a decoder reads the audio), μ-law or PCM16; ADPCM to Kiwi clients | – |
| RF attenuator from the UI | ✓ | ✓ (Kiwi-2) | ✓ admin page (`[radio] atten_db`, live), shown to listeners; HPSDR | – |
| S-meter calibration, overload mute | ✓ | ✓ | ~ `[kiwi] smeter_offset_db` (provisional -12 dB, not measured); no mute | **R** (measure; overload flag) |
| Frequency offset (converters, transverters) | ✓ | ✓ | ✓ `[radio] freq_offset_hz`: the page and the skimmers' spots (uploads) on air; Kiwi/HPSDR clients see the receiver's own | – |
| DRM (Dream + FDK-AAC) | ✓ | ✓ | ✗ | **D** (out of process; A9 CPU?) |

### 2.2 Waterfall and spectrum

| feature | stock | Kiwi | ng | plan |
|---|---|---|---|---|
| Zoom, pan, click-tune | ✓ | ✓ | ✓ | – |
| Waterfall per user / views | share or exclusive | ✓ 8 ch (v1.900) | ✓ every user their own view: the channelizer's power line and zoom engine (16 distinct zooms at 20 lines/s); shared/time-shared views on the standard bitstream | – |
| Full-band waterfall for everyone at once | ~ (zoom 0 takes one of the 2 slots) | ~ (one of its channels) | ✓ (channelizer: the power line, 0-61 MHz, any number of viewers) | – |
| Auto levels | ✓ | ✓ | ✓ | – |
| Colormaps (several + editor) | ✓ | ✓ | ✓ five (Default, Viridis, Heat, Phosphor, Grey), no editor | – |
| Spectrum peak hold, audio (AF) spectrum | ✓ | ✓ | ✓ peak hold; AF spectrum and waterfall panel (both sides of the dial in IQ) | – |
| Timestamps on the waterfall, save as image | ✓ | ✓ | ✓ UTC marks; PNG with marks and a caption | – |
| Window/interpolation/averaging choices | ✓ | ✓ | fixed (Welch) | omit (ours averages) |
| Waterfall compression | ✓ ADPCM | ✓ | ✓ lossless Rice-coded lines (55 % of raw), a data saver | – |

### 2.3 Listener UI

| feature | stock | Kiwi | OWRX+ | ng | plan |
|---|---|---|---|---|---|
| Band bars + band plan (ITU regions) | ✓ | ✓ | ✓ | ✓ band ribbon (IARU R1-3, ITU services), the phone's band picker | – |
| DX labels: own, EiBi schedules, community | ✓ | ✓ | bookmarks | ✓ own (admin upload), EiBi on air now, utility and amateur lists (v0.7.0); live DX cluster spots from Spothole (POTA/SOTA/... configurable); no community list | – |
| Frequency memories, VFO A/B | ✓ | ✓ | ✓ | ✓ in the browser, export/import JSON; A/B, A=B | – |
| Audio recording (WAV), squelch-triggered | ✓ | ✓ | MP3 | ✓ WAV in the browser, silence (squelch) left out on request; IQ recording too | – |
| Queue / camp on a user's channel | ✓ | ✓ | ✗ | ✓ (listen-along) | – |
| Users on the scale | list | list | ✗ | ✓ markers, names (callsign/nickname), list, private users | – |
| Chat | ✗ | ✗ | ✓ | ✗ | omit |
| Mobile layout, keyboard shortcuts, URL state | ✓ | ✓ | ✓ | ✓ phone layout (PR #1), keys (u/l/c/a/f/v/m, arrows) and their help (?), `#f=&m=&z=&c=` links, tuning steps, hover preview | – |
| Themes | ✓ | ~ | ✓ | ✗ (dark) | later |
| Map (receivers, spots, aircraft) | ✗ | TDoA map | ✓ | ✓ skimmer spots, the browser's own decodes and live DX spots with great circle lines; great circle (azimuthal equidistant), globe or Mercator, greyline; on phones too (tap, tap again to tune) (no aircraft, no other receivers) | **M** (more layers) |
| Languages | ✗ | ✗ | ✗ | ✗ | omit |
| Propagation forecast | ✗ | ✗ | ✗ | ✓ VOACAP (hfcast) from 7 regions, per band and UTC hour, SSB/CW/FT8; NOAA's sunspot number when online, a built-in table offline (`m2-web-ui.md` §13) | – |

### 2.4 Decoders

| decoder | stock | Kiwi | OWRX+ | ng | plan |
|---|---|---|---|---|---|
| FT8/FT4/FST4W autorun → PSKReporter, MQTT | ✓ | ✓ | ✓ | ✓ FT8/FT4 (M4; on a unit: FT8 on ten bands, ~800 spots/min), FST4W not done | FST4W if asked |
| WSPR autorun → WSPRnet, MQTT | ✓ | ✓ | ✓ | ✓ (M4) | – |
| Autorun pre-empted by users, resumes after | ~ (stock #55: doesn't resume) | ✓ | – | ✓ skimmers yield and resume (`skimmers_yield`); with the channelizer's 32 receivers they rarely have to | – |
| JTTY (Finnish teletype) | ✗ | ✗ | ✗ | ✓ in the browser (v0.6.0, WASM); not as a skimmer (the A9 is 4x too slow) | – |
| FT8/FT4/WSPR on the user's frequency, in the browser | ✗ | ✗ | ✓ (server-side) | ✓ (WASM, the skimmers' decoders; not reported; `m4-skimmers.md` §10) | – |
| CW decoder (single) | ✓ | ✓ | ✓ | ✓ every signal in the passband, classical or neural (browser) | – |
| CW skimmer (multi-signal; RBN?) | ✓ | ✓ exp. | ✗ | ~ in the browser over a 40 kHz bin (wide skimming); not on the unit (A9: 5.7x real time a busy bin) | later (off the unit) |
| FSK/RTTY, NAVTEX, DSC, SITOR | ✓ | ✓ | ✓ | ✓ RTTY/FSK (parameters detected), NAVTEX, DSC; no SITOR-A | – |
| HF FAX, SSTV | ✓ | ✓ | ✓ background | ✓ in the browser (fax APT/phasing/slant; SSTV Martin, Scottie, Robot, PD) | – |
| Time codes (DCF77, MSF, WWVB, …) | ✓ | ✓ | ✗ | ✓ DCF77, MSF, WWVB, JJY, WWV/WWVH (browser) | – |
| HFDL | ✓ (opt.) | ✓ | ✓ | ✓ in the browser (own decoder), aircraft on the map | – |
| ALE 2G | ✓ | ✓ | ✗ | ✗ | later |
| AIS, ACARS (VHF airband mode) | ✓ | ✗ | ✓ | ✗ (ACARS 131.5 MHz is in the VHF range; AIS 162 MHz is not, at 98.304 MHz) | with **V** |
| MFSK (digi_modes), STANAG-4285, Loran-C, IBP scan | ✓ | ✓ | ~ | ✗ | omit (IBP: maybe as a UI tool) |
| S-meter graph, IQ display, space weather | ✓ | ✓ | ✗ | ✗ | later (small, client side) |

### 2.5 Clients, APIs and network

| feature | stock | Kiwi | ng | plan |
|---|---|---|---|---|
| Kiwi WebSocket protocol (kiwirecorder, wsprdaemon, SuperSDR) | ✓ | ✓ | ✓ (M5, v0.8.0: SND PCM/ADPCM/IQ, W/F; kiwirecorder and SuperSDR tried) | – (wsprdaemon not tried) |
| `/status`-style listing data, `/snr`, `/users` | ✓ | ✓ | ~ `/status`, `/VER`, `/api/status`, metrics; no `/snr`, `/users` | with the listing question (§5) |
| HPSDR P1 | ✗ (not built) | ✗ | ✓ | – |
| IQ stream / rtl_tcp / SpyServer | ✗ (dormant) | ✗ | ✗ | later (design §7) |
| MQTT (spots, start/stop, temperature) | ✓ | ✗ | ✓ spots, band start/stop, uploads (M4; no temperature; CPU and temperature are in the status bar and metrics) | – |
| Public listing (rx-888.com / rx.kiwisdr.com) | ✓ rx-888.com | ✓ | ✗ | open question |
| Vendor reverse proxy (frpc), DUC/no-ip, UPnP | ✓ | ✓ | ✓ tunnels from `web888.txt`: frp (the first outside PR), cloudflared, rathole, chisel, to the owner's server; no vendor service, DUC or UPnP | – |
| ACME certificates | ✓ | ✗ | ✗ (self-signed/files) | later |
| TDoA (GPS-timestamped IQ, Kiwi TDoA network) | ✗ (disabled) | ✓ | ✗ | after **K** + open FPGA timestamps |

### 2.6 Admin and operations

| feature | stock | Kiwi | ng | plan |
|---|---|---|---|---|
| Status, users + kick, config, passwords | ✓ | ✓ | ✓ | – |
| Session time limit, inactivity limit | ✓ | ✓ | ✓ (fixed 2026-10-23) | – |
| Client address behind a reverse proxy (trusted proxies, X-Forwarded-For) | ✓ | ✓ | ✓ `[web] trusted_proxies` (fixed 2026-10-23) | – |
| Per-IP daily limit, IP block list, ban from the user list | ✓ | ✓ | ✓ `daily_minutes`, `blocked`, Ban in the admin's Sessions | – |
| Channels without a password (open + private sets) | ✓ | ✓ | ✓ `open_channels`: visitors on that many, the password for any (web and Kiwi) | – |
| Receiver info page (name, location, antenna, photo) | ✓ | ✓ | ✓ Info: name, place, station, antenna, description, contact, photo, the limits | – |
| Logs in the admin page | ✓ | ✓ | ✓ (v0.13.0: the journal, the setup log) | – |
| Updates (channels, rollback) | ✓ zip | ✓ | ✓ os: A/B with rollback (RAUC, since os v0.3.0); the admin page looks for a release (daily too), installs it with progress and reboots into it | – |
| Network config (static IP, Wi-Fi) | ✓ | ✓ | ✓ os: DHCP or static from `web888.txt` (the board has no Wi-Fi) | – |
| Usage analytics (heatmaps, visitors) | ✓ | ~ | metrics | omit (Prometheus/Grafana) |
| Browser shell console | ✓ | ✓ | ✗ | omit (ssh) |
| Backup / restore | ✓ SD | ✓ | ✓ settings backup/restore from the admin page (v0.13.0); the config on its own partition | – |

### 2.7 Hardware modes (with the hw repo)

| feature | stock | ng | plan |
|---|---|---|---|
| HF 0–62 MHz at 122.88 MHz | ✓ | ✓ (the channelizer by default since os v0.7.0; the tuned HF/VHF bitstreams frozen at hw v0.5.0) | – |
| 32 MHz "narrowband" (61.44 MHz ADC) | ✓ | ✗ | omit unless asked |
| VHF airband by undersampling (98.304 / 110.592 MHz clocks, RF switch) | ✓ | ~ **experimental** (v0.10.0, `band = "vhf"`): 98.304-147.456 MHz on the VHF channelizer, NBFM; ran on a unit with no VHF antenna; 110.592 MHz (to 165.9 MHz, marine VHF) only with the frozen tuned bitstream | **V** (real signals; 110.592 MHz with the channelizer?) |
| External 10 MHz reference, GPSDO output on CLK2 | ✓ | ✗ (the RF switch and clock select are in the driver; nothing configures the reference) | **V** |
| ADC dither, PGA, DC-offset trim | ✓ | ✓ dither and PGA (admin Status, live); no DC trim (the channelizer has no DC term) | – |
| GPS: position via gpsd, PPS clock correction | ✓ | PPS loop ✓, gpsd ✓ (M4) | **S** (grid for spots) |

## 3. Order

### 3.1 Done (2026-10-23 to 2026-11-12)

* **Fix now**: session and idle limits, trusted proxies (2026-10-23).
* **S** (M4): FT8/FT4/WSPR skimmers, uploads, MQTT/syslog, gpsd, spots in
  the UI; the decoders in process at `SCHED_IDLE` with a deadline per
  slot instead of child processes. FST4W left.
* **K** (M5): the Kiwi protocol; kiwirecorder and SuperSDR work.
* **Open FPGA** (M6): the channelizer with 32 tuner receivers, the
  power line and the zoom engine; the os default since v0.7.0. This
  answered the A9 cost questions of the old step B: the FPGA delivers 12
  kS/s receivers and the zoomed spectra.
* **R, in part**: SAM family, NBFM, band ribbon and plan, own + EiBi +
  utility labels, the phone layout, URL state, tuning steps, the HTTP
  audio fallback.
* **B**: JTTY, and FT8/FT4/WSPR on the listener's own frequency, in the
  browser.
* **O, the os side**: A/B updates with rollback, network settings,
  tunnels; OS updates from the admin page; Prometheus metrics with a
  Grafana dashboard.
* **M, a first map**: skimmer spots, the browser's decodes and live DX
  cluster spots (Spothole), also in the spot list.
* **V, a first step**: VHF by undersampling, experimental.

### 3.2 Proposed next

1. **V-check — VHF on real signals** (needs an antenna or a signal
   generator on the VHF connector): levels, the input filter's images
   (FM broadcast), NBFM and AM audio. Small in work, decides whether
   VHF leaves "experimental".
2. **R — the rest of receiver parity** (done 2026-11-17: frequency
   offset, AGC hang/decay, memories and VFO A/B, WAV recording,
   colormaps, peak hold, waterfall time marks and PNG, key help; before:
   the attenuator, 24 kS/s audio, Opus/μ-law, the auto-notch):
   * a measured S-meter calibration (dBm) for both the web UI and Kiwi
     (waits for a signal source: the second unit).
   (Also done 2026-11-17: ADC dither/PGA, IQ in the web UI, the audio
   spectrum.)
3. **O — access and presentation**: done 2026-11-17 (per-IP daily
   limit, block list and Ban, open channels for visitors, the info
   page; logs and settings backup in v0.13.0). Left: a public listing
   (§5).
4. **B — more decoders in the browser**: done (M8: CW, RTTY/NAVTEX/DSC,
   PSK, FAX/SSTV, HFDL, JS8, time codes, the signal identifier); what is
   left is in `m8-browser-decoders.md`.
5. **M — more map layers**: EiBi transmitters on air, other receivers.
6. **D — heavy server-side decoders**: DRM (FDK-AAC strictly out of
   process); measure the A9 first. (HFDL went to the browser instead.)
7. **V — the rest**: the external 10 MHz reference, ACARS on VHF,
   110.592 MHz on the channelizer (a fractional rate) if marine VHF is
   wanted.
8. **P — propagation**: done 2026-11-17 (the VOACAP panel, §13 of
   `m2-web-ui.md`). Next steps there: the band ribbon shaded by the
   forecast, a "now" view from the skimmers' own spots beside it.
9. **Later**: S2 (CW/RTTY skimmers, off the unit), FST4W, GPS
   timestamps and TDoA (M6's open item; the second unit has a GPS
   antenna), IQ stream/rtl_tcp/SpyServer, ACME, themes, ALE, public
   listing (§5).

**Omissions** (unchanged unless §5 says otherwise):
* languages (chat: see §5)
* C-QUAM, MFSK/STANAG-4285/Loran-C
* waterfall window and interpolation choices (ours averages)
* DUC and UPnP (tunnels instead), the vendor's proxy service
* the browser console (ssh instead)
* usage analytics (metrics instead)
* 32 MHz narrowband mode

## 4. OpenWebRX and OpenWebRX+

**Model.**
* OWRX shares one SDR "profile" (centre frequency and rate) between all
  users of a device, with one waterfall. Changing it moves everyone. The
  lock-profile option, the "magic key" and the bot bans exist because of
  that.
* Each user gets a server-side demodulation chain decimating from the full
  device rate. CPU per user is high: a few users on a Raspberry Pi 3.
* web888-ng's per-user FPGA channels and shared views already avoid all
  of this, so nothing from that side is needed.

**What OWRX+ has that the Kiwi family doesn't, and how it would fit here:**

| idea | what it is in OWRX+ | here | plan |
|---|---|---|---|
| Decoder scheduler: static, or daylight/greyline profiles | Background services follow a schedule when nobody listens | ✓ skimmer leases by UTC hours (M4, `[[skimmer]] hours`); daylight/greyline profiles not yet | later |
| CW and RTTY skimmers to PSKReporter | csdr-skimmer: every signal in a 96 kHz slice, callsigns spotted | One 48 kHz channel per band segment. Kiwi's CW skimmer is interactive only and spots nothing | **S2** (after FT8/WSPR; A9 CPU to measure) |
| MQTT event stream: spots, connects, channel use; federation between servers | Per-mode topics, JSON | ✓ spots and skimmer events (M4); connects and channel use not yet | later (federation too) |
| Map | Locators with QSO/call lines, APRS, aircraft from HFDL/VDL2/ACARS, ships, other web SDRs with user counts, EiBi transmitters on air (click to tune) | Spots (S) and EiBi (labels) exist now; HFDL aircraft would come from D; Leaflet in the browser, server sends JSON | **M** |
| Files gallery | SSTV/FAX images, NAVTEX/text logs, squelch-triggered MP3s, with a keep limit | Needs storage: the config partition is 32 MB, so use RAM with a cap or a USB stick | with **B**/**D** |
| Scheduled squelched audio logger | MP3 per channel with SNR squelch and hang | e.g. VOLMET or a net; Opus at 12 kHz is cheap | later |
| Bookmarks + scanner | Auto-bookmarks from EiBi (in range) and band plans; scanner steps through visible bookmarks with squelch | The scanner is client-side retuning; the server has squelch and power | **R** |
| Chat, admin broadcast | Random nicknames, timestamps, federated over MQTT | Small. Moved from "omit" to a question (§5) | ? |
| Abuse protection | Robot score from connection rate and rapid profile changes, timed IP bans, trusted proxies, session timeout with a policy page, LAN-only admin login | Trusted proxies, IP bans (`blocked`, Ban), a daily limit and session/idle limits (shown on the info page) done; no robot score or timed bans | later |
| Opus audio | Wanted for years (upstream #220, a 2022 WebCodecs branch abandoned) | Done: Opus (pure-Rust encoder, WASM decoder in the page), μ-law, PCM | – |
| Whisper speech-to-text | 20 s WAV chunks POSTed to an external Whisper server; transcripts to the page and MQTT | No inference on the unit; an optional URL in the config. Tried offline on our audio (2026-10-29): AM usable, SSB poor, hallucinations without voice detection | later (AM broadcast / VHF AM first) |
| FreeDV (700D/E, RADE) | Server-side decoder per user | HF digital voice. Better as WASM in the browser (RADE is a neural network) | later, browser |
| Selcall/DTMF/EAS, POCSAG, ISM, AIS, ADS-B, DAB, HD Radio, LoRa, radiosondes | Many decoders, mostly VHF/UHF | Mostly outside our bands (HF, and 98.3-147.5 MHz on VHF); ACARS fits the VHF range | omit (except DSC in **B**, ACARS with **V**) |
| Secondary audio-passband waterfall for digimodes (click sets the decoder offset) | Panels for text, tables and images | The browser-side decoder UI | **B** |
| Keyboard help overlay, themes, Hz pointer readout, pinch zoom, log volume, saved passband per mode | Many small UI items | Cheap, client side; key help, pinch zoom and the hover readout done | **R** (the rest) |
| Rig follow (Hamlib) | Radio follows the receiver, optional PTT | Private stations only | later |
| JavaScript plugin API | Buttons, windows and panels from plugins | Against "small, owned surface" (design §1) for now | omit for now |

## 5. Open questions

Settled since 2026-10-22: skimmers first (yes, M4); R or K first (K came
first, M5); VHF wanted (yes: experimental in v0.10.0); interactive
decoders in the browser (yes, JTTY was the first).

1. **The next order** (§3.2): VHF on real signals, then R, O, B, M, D?
   Or O first, if the unit is to be a public receiver soon?
2. **Public listing**: register at rx-888.com like stock (their API), at
   rx.kiwisdr.com (the Kiwi protocol is there now; needs `/snr` and
   `/users`), or nothing for now?
3. **VHF**: is 98.304-147.456 MHz enough (airband, 2 m, ACARS), or is
   marine VHF (156-162 MHz) worth a 110.592 MHz channelizer (54 kS/s
   bins: a fractional resampler)?
4. Anything in the omit list that you use?
5. Chat? Whisper through an external server (§4)? (The map is done.)
6. CW/RTTY skimmers (**S2**): the channelizer freed the CPU that the
   receivers took; worth measuring the A9 for them now?
7. FST4W: wanted by anyone?
