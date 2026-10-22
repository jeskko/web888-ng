# M2 web UI: implementation plan

Status: **agreed** (2026-10-22; decisions in §10). Scope from `design.md` §11 M2:
web UI MVP in a browser for one user, with an exclusive waterfall,
SSB/AM/CW audio and tuning. Design decisions already made in `design.md` §7
(axum, separate `ui/` package in TypeScript, JSON control plus binary
frames, WebGL waterfall, AudioWorklet, PCM16 first) are not reopened here.

Done when: on the workstation, `web888d serve --backend iq` with the HackRF
(or a recording) gives a usable receiver in Chrome and Firefox: the
waterfall pans and zooms, click to tune, USB/LSB/CW/AM audio without
dropouts, an S-meter, and HPSDR keeps working at the same time. The same
binary and UI run in the image in the emulator (sim/model signals).

---

## 1. Process and threads

```
                       ┌─ HwCmd (mpsc) ◀── hpsdr thread (Control → HwCmd)
                       │               ◀── web session (tokio)
 Radio ──▶ [hw thread] ┤
                       ├─ Arc<RxBlock> ─▶ hpsdr thread           (bounded, try_send)
                       ├─ Arc<RxBlock> ─▶ [audio thread] ─ AudioFrame ─▶ ws task
                       └─ Arc<WfBlock> ─▶ [wf thread]    ─ WfFrame    ─▶ ws task
                                                    (tokio mpsc, try_send; drops counted)
 tokio runtime (2 workers): axum HTTP(S), static files, /ws session, /api/status
```

* **hw thread** (existing `hw.rs`, extended). It stays the only owner of
  the `Radio`. Changes:
  * The command enum is now `HwCmd { Tune{ch,hz}, Atten{db}, WfStart{slot,hz,decim},
    WfStop{slot} }`. HPSDR's `Control` is translated at its sender, so the
    hpsdr crate does not depend on web888d.
  * RX fan-out: a list of subscribers (`SyncSender<Arc<RxBlock>>`), each
    fed with `try_send`, so a slow consumer loses blocks (counted per
    subscriber) and never stalls the others. `RxBlock { first_frame: u64,
    nch, iq: Vec<Iq32>, overload }` moves to `web888-radio`. `first_frame`
    lets consumers see gaps. The hpsdr crate switches to `Arc<RxBlock>`
    (a small change, and it drops a per-block copy).
  * WF: while a slot is active (continuous mode), `read_wf` runs in the same
    loop. Samples go out as `Arc<WfBlock { slot, gen, contiguous, iq: Vec<Iq16> }>`.
    `gen` increments on every `WfStart`, so the wf thread drops stale
    samples after a retune. `contiguous = false` marks a gap: always at the
    block boundaries on the stock driver (back-to-back one-shots), and after
    a drop. FFTs never span a gap.
  * PPS retune covers the active WF slot too (restart with the new NCO word).
* **audio thread**: one per web session in M2 (a pool comes with M3
  leases). DSP in §3.1.
* **wf thread**: one, serving slot 0. DSP in §3.2.
* **tokio**: `multi_thread` with 2 workers, below the DSP threads in
  priority. No sample processing happens on the runtime.

Channel assignment for M2: the web user gets **the last RX channel**
(`--web-rx` default `rx_channels - 1`, i.e. 11 on HF). HPSDR keeps
`0..max_rx` with `max_rx = min(7, rx_channels - 1)`. The hardware rate
stays at 48 kS/s, which HPSDR needs. The 24 kS/s default question
(`design.md` §13.1) waits for M3, when leases make the rate a policy.

Single user: a second `/ws` connection gets `{"type":"busy"}` and is
closed with code 1013. Taking over from the existing user is an M3
feature (it needs auth).

## 2. Radio API additions

`Caps` gains:

* `rf_range: RangeInclusive<f64>`: tunable range. HF is 0..=61.44 MHz, VHF
  depends on the variant, and iq is `centre ± 0.45·rate`. The UI's full view
  is this range.
* `wf_decim_min: u16`: 1 on hardware and sim, the input divisor on iq (16 at
  7.68 MS/s). Today iq refuses smaller values, but nothing advertises the
  limit.

## 3. DSP (new code in `crates/dsp`, unit-tested there)

### 3.1 Audio chain (48 kS/s → 12 kS/s)

```
RxBlock ─▶ pick channel ─▶ rx_to_c32 ─▶ decimate ×4 (FIR, flat to ±5.5 kHz)
  ─▶ channel filter @ 12k (OLS, design() with Droop::Rx, per mode)
  ─▶ CW: shift +pitch ─▶ demod (ssb: re / Am) ─▶ AGC ─▶ volume ─▶ i16
  ─▶ 20 ms frames (240 samples) + RSSI
```

* The pre-decimation filter comes first: the sharp channel filter at 12 k
  needs about 4× fewer taps (and runs on 4× fewer samples) than at 48 k.
  `Droop::Rx` depends only on f, so `design()` works at 12 k unchanged.
* Modes and default passbands: USB 300..2700, LSB −2700..−300, CW ±250
  around the carrier with the pitch shift (default 700 Hz), AM ±5000. The
  UI sends `lo/hi`; edges are clamped to ±5 kHz (the pre-decimation filter
  is flat to 5.3 kHz, 160 taps). The channel filter's overlap-save FFT is
  sized for about 60 ms blocks (latency) rather than the default 150 ms.
  On a filter change, the new taps are designed on the audio thread
  (a few ms) and swapped in between blocks.
* Tuning: the hardware NCO is set to the dial frequency (the carrier). CW
  shifts in software only, as in `design.md` §5.1.
* **AGC** (an M2 minimum, refined in M3): a peak follower with
  1 ms attack, a hang time (SSB/CW 300 ms, AM 0) and then a 20 dB/s decay.
  It has a max gain and an "off + manual gain" setting. The RSSI is the
  channel power (dBFS) before AGC, averaged over the frame.
* Gaps: a jump in `first_frame` sets the next audio frame's gap flag
  (the UI counts these). The audio is not patched over.

### 3.2 Waterfall engine (Welch, as in `design.md` §5.2)

* Input: WF slot 0 continuous, `fs_wf = adc/decim`, span = `fs_wf`
  (complex).
* FFT size `N = max(1024, next_pow2(2·bins))`, ≤ 8192. Window:
  4-term Blackman-Harris. Hop: N/2.
* Per frame interval (1/fps): sum |X|² over all FFTs that fit, capped by a
  **budget** (FFTs per frame, `--wf-budget`, adaptive later). The engine
  streams with bounded memory: an FFT every `stride` samples, stride =
  max(N/2, samples in the previous line / budget), so above the budget the
  FFTs spread evenly over the interval instead of taking only the start.
* Narrow spans (fewer than N/2 new samples per interval): the hop shrinks
  so that each frame still has ≥ 1 FFT over the newest N samples (a
  sliding window). Frames never wait longer than 1/fps.
* Then: normalise (a full-scale sine reads 0 dBFS in any bin size), correct
  the WF CIC droop per bin (`wf_chain_mag`, precomputed per decim),
  fftshift, then merge to `bins` by **averaging power** (peak view is a
  UI option later), and quantise to u8 dB (§4.3).
* Test: the impulsive-noise criterion from `design.md` §5.2 (line-to-line
  spread within 0.3 dB of stationary noise). This is a dsp test with a
  synthetic 100 Hz-gated noise burst, not the `web888-pl` scene.
* Cost on the A9: about 0.25 ms per complex 2048-point FFT. With
  20 fps × 30 FFTs that is about 15 % of one core. The default budget is
  set so that WF + one audio channel + HPSDR stay below 50 % of one core.
  Measure on the board, not in the emulator.

## 4. WebSocket protocol (`/ws`, version 1)

Text frames carry JSON control (both directions); binary frames carry data
(server → client only). All binary fields are little endian. The Rust types
live in one module (`crates/web/src/proto.rs`) and the TypeScript mirror is
in `ui/src/proto.ts`. A round-trip test checks the JSON against fixtures in
`crates/web/tests/proto/` that both sides use.

### 4.1 Client → server

| type | fields | notes |
|---|---|---|
| `hello` | `proto: 1`, `codecs: ["pcm16"]` | first message |
| `tune` | `hz` | dial frequency; clamped to `rf_range` |
| `mode` | `mode: usb\|lsb\|cw\|am`, `lo?`, `hi?`, `pitch?` | passband relative to the dial |
| `audio` | `agc: {on, max_gain_db?}`, `gain_db?`, `mute?` | |
| `wf` | `center_hz`, `decim`, `bins`, `fps` | the server clamps and echoes the values it used |
| `ping` | `t` | the server answers `pong` (latency shown in the status bar) |

### 4.2 Server → client

| type | fields |
|---|---|
| `hello` | `proto`, `server` (version), `codec` (chosen), `audio_rate`, `radio: {backend, variant, adc_hz, rx_rate, rf_lo, rf_hi, wf_decim_min, wf_decim_max}`, `limits: {bins_max, fps_max}` |
| `state` | the full authoritative state after any change: `hz`, `mode`, `lo`, `hi`, `pitch`, `agc`, `gain_db`, `wf {center_hz, decim, bins, fps}` |
| `status` | 1 Hz: `adc_overloads`, `clock_ppm`, `audio_drops`, `wf_drops`, `hw_rate_ok` |
| `busy`, `error` | `msg` |

### 4.3 Binary frames

Audio (`kind = 1`), every 20 ms:

| off | type | field |
|---|---|---|
| 0 | u8 | kind = 1 |
| 1 | u8 | flags: bit0 ADC overload, bit1 gap before this frame |
| 2 | u16 | n samples |
| 4 | u32 | seq |
| 8 | u32 | sample rate |
| 12 | f32 | RSSI, dBFS |
| 16 | i16 × n | PCM16 mono |

Waterfall (`kind = 2`), one line:

| off | type | field |
|---|---|---|
| 0 | u8 | kind = 2 |
| 1 | u8 | flags: bit0 ADC overload, bit1 new `gen` (retune/zoom) |
| 2 | u16 | bins |
| 4 | u32 | seq |
| 8 | f64 | frequency of the first bin's centre (Hz) |
| 16 | f64 | bin width (Hz) |
| 24 | f32 | dB of value 0 (dBFS) |
| 28 | f32 | dB per step (160/255: −160..0 dBFS) |
| 32 | f64 | time, Unix ms (of the end of the interval) |
| 40 | u8 × bins | levels |

Every line carries its own frequency mapping, so the UI keeps history
aligned while panning and zooming (§5.2), and the UI colour range
(contrast) is applied client-side.

Bandwidth: audio 24 kB/s, WF 1024 bins × 20 fps ≈ 21 kB/s.

## 5. UI (`ui/`, TypeScript, GPL-3.0+)

### 5.1 Tooling

* `pnpm`: dev dependencies `typescript` and `esbuild`; the only runtime
  dependency is **Preact** (MIT, §10.2) for the controls (and, in M3, the
  admin/config pages). The waterfall, spectrum and worklet stay plain
  TypeScript. `pnpm build`
  runs `tsc --noEmit` and esbuild into `ui/dist/` (index.html, app.js,
  worklet.js, style.css; hashed names not needed). `pnpm dev` is esbuild
  watch.
* Tests: `node --test` on the pure modules (protocol parsing against the
  shared fixtures, frequency formatting/stepping, resampler).

### 5.2 Structure

```
ui/src/
  main.ts              boot, start overlay (AudioContext needs a click), layout
  proto.ts             message types, binary frame parsers
  net.ts               WebSocket, reconnect with backoff, ping
  state.ts             small observable store: radio, tuning, view
  view/waterfall.ts    WebGL2: R8 history texture (ring of lines × bins),
                       per-line (f0, df) in a float texture, colormap LUT,
                       contrast uniforms; scroll = ring offset
  view/spectrum.ts     Canvas 2D: latest line + slow average, peak hold
  view/scale.ts        frequency axis, passband overlay, tuning marker
  ctl/*.tsx are Preact components; view/ and audio/ are plain TS.
  ctl/freq.tsx         frequency display: per-digit wheel/keys, typed entry
  ctl/panel.tsx        mode, bandwidth, AGC/gain, volume/mute, zoom, contrast
  ctl/smeter.tsx       RSSI bar + dBFS readout, overload lamp
  audio/player.ts      AudioContext, worklet node, volume
  audio/worklet.ts     ring buffer + adaptive resampler 12k → context rate
```

Behaviour:

* The waterfall shader maps each screen pixel's frequency through **that
  line's** (f0, df). Old lines therefore stay in place when the view pans
  or zooms (they show blank outside their own span) instead of being
  smeared or cleared.
* Wheel on the waterfall: zoom around the cursor (power-of-two decimation
  steps, `wf_decim_min..max`). Drag: pan (WF NCO retune). Click: tune
  (snapped to 10 Hz, or 1 kHz in AM). Shift+wheel: tune step. Keyboard:
  arrows step, `u/l/c/a` select the mode.
* The URL hash keeps the frequency, mode and zoom (`#f=7074.0&m=usb&z=6`),
  so links reopen the same view.
* Audio: the worklet keeps a jitter buffer with a target of 150 ms. The
  resampling ratio is trimmed by a slow loop on the buffer fill (sound card
  vs. ADC clock drift). Underrun gives silence and a counter, never a
  replay. Linear interpolation first; a polyphase filter later if it is
  audible.
* Dark theme. The layout works down to a phone width, with the controls
  under the waterfall.

## 6. HTTP server (`crates/web`, crate `web888-web`)

* axum: `GET /` static from `--ui-dir` (default `/usr/share/web888d/ui`,
  `tower-http` ServeDir, gzip), `GET /ws`, `GET /api/status` (JSON, the
  same content as `status`).
* Listen: `--http 0.0.0.0:8073` (the stock server's port, so existing
  bookmarks and reverse proxies keep working). TLS on the same port
  (§10.1): the first bytes of a connection tell a TLS ClientHello from
  plain HTTP, so one port serves both. The certificate comes from
  `--tls-cert/--tls-key`, else a self-signed one generated on first start
  (rcgen, SANs: hostname, `localhost`, the interface addresses) and kept in
  `--state-dir` (default `/var/lib/web888d`). `--no-tls` serves plain only.
* Session: the tokio task holds the WS. Control goes from JSON to `HwCmd`
  (to the hw thread) or to a `SessionCmd` (to the audio and wf threads,
  over crossbeam channels). Data comes back through tokio mpsc.
* web888d wiring: `serve` starts hw, hpsdr, web (`--no-web` and
  `--no-hpsdr` to switch either off).
* Dependencies (all MIT/Apache, GPL-3.0-compatible): axum, tokio,
  tower-http, serde, serde_json, crossbeam-channel, tokio-rustls (ring
  provider; aws-lc-rs not needed) and rcgen.

## 7. Packaging

* web888d itself: new crates → regenerate `web888d-crates.inc` in the os
  recipe (as now).
* `ui/dist` into the image (question 3). Recommended: the server CI
  builds `web888d-ui-<version>.tar.gz` on `v*` tags (release + SHA256SUMS,
  like the hw bitstreams). A new os recipe `web888d-ui` fetches it by
  checksum into `/usr/share/web888d/ui`. Yocto then needs no Node, and dev
  images between tags can use a locally built tarball via
  `SRC_URI:append` in `local.conf`.

## 8. Development and test

* Workstation:
  `pnpm -C ui dev` plus `cargo run --release -p web888d --features soapy --
  serve --backend iq --iq soapy:driver=hackrf --iq-freq 8.6e6 ... --ui-dir ui/dist`,
  then `http://localhost:8073` (localhost counts as a secure context, so
  AudioWorklet works).
* The 3 s recording loops too often for UI work. Record about 60 s
  (≈ 920 MB cs8) with the same settings into `/data/iq`.
* Rust integration test (`crates/web/tests/session.rs`): sim backend
  (tone 7.0755 MHz at −40 dBFS) and a tokio-tungstenite client. Checks:
  `hello`, then tune 7.074 MHz USB → the audio's dominant tone is
  1500 ± 20 Hz, a WF line has its peak in the bin of 7.0755 MHz, and a
  second client gets `busy`. Runs in `cargo test --workspace`.
* Emulator: the image boots into web888d. Forward 8073 and check that the
  UI loads and the model's carriers appear in the waterfall (the model
  supports WF).

## 9. Steps (each ends in a commit)

| # | content | done when |
|---|---|---|
| 1 | hw loop: `HwCmd`, `RxBlock` fan-out, WF reading, `Caps` additions; hpsdr on `Arc<RxBlock>` | existing tests + p1probe against sim and iq unchanged |
| 2 | dsp: decimator, AGC, CW shift, Welch WF engine | unit tests incl. impulsive-noise criterion |
| 3 | `crates/web`: axum, proto, session, audio + wf threads; wiring in `serve` | session integration test passes |
| 4 | `ui/`: tooling, net/proto, waterfall + spectrum + scale, tuning | waterfall live from HackRF, pan/zoom/click |
| 5 | audio: worklet, modes/BW, AGC/gain, S-meter, URL state | listen to FT8/SSB/AM on 40 m with no dropouts over 10 min |
| 6 | TLS (per question 1), CI UI tarball, os recipe, README/design updates | image in the emulator serves the UI |

## 10. Decisions (reviewed 2026-10-22)

1. **TLS:** built-in, self-signed certificate for now (generated on first
   start, one browser warning per client); configured certificates and
   plain HTTP for a TLS-terminating proxy also work (§6).
2. **UI framework:** Preact, for the controls now and the admin/config
   pages in M3.
3. **Packaging:** CI tarball on `v*` tags + os recipe `web888d-ui` (§7).
4. **Port:** 8073 by default.
