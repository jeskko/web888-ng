# web888-ng: new server (`web888d`) for the Web-888 SDR

A Rust server for the Web-888 receiver: its own web UI, standard SDR
protocols (HPSDR, rtl_tcp, IQ), decoders, and KiwiSDR protocol
compatibility. It runs on the stock firmware (stock or open bitstream) and
later on the open driver from [web888-ng-hw](https://github.com/jeskko/web888-ng-hw).

Status (see `doc/design.md` §11): **M1** stock-driver backend and HPSDR, **M2** web UI, **M3** multi-user and **M4** skimmers (FT8, FT4, WSPR; PSKReporter, WSPRnet, syslog, MQTT) are done. The stock-driver backend has run on a production unit, with the stock and with the open bitstream (2026-10-28). Next: a channelizer bitstream (M6, `hw/docs/channelizer.md` in web888-ng-hw).

| crate | content |
|---|---|
| `crates/radio` | `Radio` trait, sample types, channel math (NCO words, rates, CIC response) |
| `crates/sim` | simulated Web-888: RF scene → real channel model (NCO quantisation, clock error, CIC response, aliasing, noise), real-time or free-running, ring overrun and PPS |
| `crates/dsp` | Kaiser FIR, droop-compensated channel design, overlap-save filter, shifter, SSB/AM demod |
| `crates/zynqsdr` | backend for the stock kernel driver (`/dev/zynqsdr` ioctls), bitstream loading |
| `crates/hpsdr` | OpenHPSDR Protocol 1 receive server (discovery, up to 7 receivers, 48 kS/s) |
| `crates/hw` | the hardware thread: owns the radio, fans RX/WF blocks out to HPSDR and web sessions, time-shares WF slots |
| `crates/core` | what clients share: RX channel leases, the DSP pool, the waterfall scheduler |
| `crates/web` | web UI server: HTTPS/HTTP on one port, WebSocket sessions (audio + waterfall) |
| `crates/ftx` | FT8 and FT4 decoders |
| `crates/wspr` | WSPR-2 decoder (a port of wsprd) |
| `crates/skim` | skimmers: band jobs, decoder threads, the spot bus, PSKReporter, WSPRnet, syslog, MQTT, gpsd |
| `ui/` | the web UI (TypeScript, Preact, WebGL2 waterfall, AudioWorklet) |
| `crates/iq` | backend fed by real signals: wideband IQ from an SDR (SoapySDR, e.g. HackRF) or a recording, channelized like the FPGA |
| `crates/web888d` | the daemon: `serve` (radio + HPSDR + web UI), `sim` (one channel to stdout) |

```
cargo test --workspace
cargo run --release -p web888d -- sim --tone 7.0755e6:-40 --noise --tune 7.074e6 --mode usb \
  | aplay -t raw -f FLOAT_LE -r 24000 -c 1
```

On a unit (stock firmware), with the CI binary copied over:

```
rc-service sdrd stop      # the driver allows one opener
./web888d serve           # loads websdr_hf.bit, serves HPSDR P1 on UDP 1024
```

Then point an HPSDR client (SparkSDR, SDR++ hermes source, Thetis, ...) at
the unit at 48 kS/s. `rc-service sdrd start` brings the stock server back.

### Configuration

`web888d serve` reads `/etc/web888d/web888d.toml` (`--config`; a missing
file means all defaults). Every option has a key there, options on the
command line override the file, and `serve --print-config` prints the
result, which also serves as a template:

```toml
[radio]
backend = "iq"
[radio.iq]
source = "soapy:driver=hackrf"
freq = 8.6e6
gains = { LNA = 16, VGA = 20, AMP = 0 }
[web]
tls = "off"            # self-signed (default) | files | off
```

Changes are picked up within 2 s (or on SIGHUP): limits, auth and web
defaults apply at once; radio, HPSDR and listen/TLS settings are logged
as needing a restart. A file that doesn't parse is reported and ignored.

### Web UI

`web888d serve` also serves the web UI on port 8073: HTTPS with a
self-signed certificate (generated on first start into `--state-dir`,
default `/var/lib/web888d`; or `--tls-cert`/`--tls-key`), and plain HTTP on
the same port for a TLS-terminating proxy (`--no-tls` for plain only).
`[limits] session_minutes` and `idle_minutes` end web sessions after a
time, or after that long without input (admins exempt; a notice comes a
minute before, and the page offers a Reconnect button rather than
reconnecting by itself).
Behind a reverse proxy, list it in `[web] trusted_proxies` (addresses or
networks): web888d then takes the client's address from
`X-Forwarded-For` (and `https` from `X-Forwarded-Proto`), so per-address
limits and logins count real clients, not the proxy.
Browsers only play audio over HTTPS or from `localhost`. Web users and
HPSDR receivers share the RX channels through leases (`[limits]`:
`per_ip` sessions per address, default 4; `hpsdr_channels`); a web user
who finds them all taken gets the waterfall only, until one frees. Users
with the same waterfall view share one capture; more distinct views than
the two WF slots are time-shared (`doc/m3-multi-user.md`). Load test:
`cargo run --release -p web888-web --example webload -- ws://HOST:8073/ws`.

The UI lives in `ui/` (TypeScript + Preact, built with esbuild; needs
Node 24 and pnpm):

```
cd ui && pnpm install && pnpm build      # -> ui/dist (pnpm dev: rebuild on change)
cargo run --release -p web888d -- serve --backend iq --iq file:hf.cs8 --iq-freq 8.6e6 --iq-loop \
  --ui-dir ui/dist --state-dir /tmp/web888d
# then https://localhost:8073 (or http://localhost:8073)
```

UI checks in headless Chromium (muted): `ui/tools/cdp/` (receiver,
login, admin pages, idle limit, layout). armv7 builds of benches and
tools without a local toolchain: `scripts/cross-armv7.sh`.

Waterfall: wheel zooms around the cursor, drag pans, click tunes
(hovering shows the frequency and the passband a click would give),
shift+wheel steps the dial; arrows step, `u`/`l`/`c` pick the mode and
`a` (or the AM button, again and again) cycles AM, SAM, SAL, SAR, SAS:
synchronous AM, both sidebands, the lower or upper alone (the channel
filter removes the other), or stereo with the lower sideband left. The
tuning step (auto per mode, or 1 Hz to 10 kHz; 9 kHz for MW) applies to
the arrows, the wheel and click-tuning, all to its grid.
The view is kept in the URL (`#f=7074&m=usb&z=256&c=7100`). The scale
shows other users' frequencies (click to listen along) and each
skimmer's range. A callsign or nickname (8 characters, kept in the
browser) shows by the dial marker and to others; the Users button lists
everyone (frequencies of those who share) and the skimmers. The status
bar shows the device's CPU load and temperature. **Stereo** (USB, LSB,
CW; headphones) places signals left or right of the passband's middle,
as on the waterfall (`doc/stereo-cw.md`).
The protocol is version 2: `doc/m3-multi-user.md` §8 (version 1, M2's,
in `doc/m2-web-ui.md` §4).

### Passwords, admin API, metrics

```
web888d passwd                 # the admin password (asked twice), into the config file
web888d passwd --listener      # a shared listener password: a private receiver
web888d passwd --listener --remove
```

A running server picks the change up. A first admin password can also be
put on the SD card as `/boot/web888d-admin` (one line, at least 8
characters): it is applied at the next start and the file is deleted (also
the way back in after a forgotten password). There is no default password.

The admin pages are at `#/admin` on the receiver's address (link in its
status bar): status, sessions (kick), the configuration form, passwords.
Logins are `POST /api/login {"password": ..., "admin": true}` (a 12 h
HttpOnly cookie; 5 failed tries per address per minute). With the admin
cookie: `/api/admin/sessions` (and `.../{id}/kick`), `/api/admin/leases`,
`/api/admin/config` (get, `PUT` saves the file, `/validate` says what a
change would need), `/api/admin/password`, `/api/admin/stats` (CPU per
thread, DSP pool, waterfall views) and `POST /api/admin/restart` (exits
75; the service restarts it). `[web] metrics = true` serves Prometheus
metrics at `/metrics` (no login). Details: `doc/m3-multi-user.md` §7.

### Skimmers

Background decoders on RX channels of their own, in UTC slots, low
priority. Their spots show in the UI (a list, labels on the scale, each
skimmer's range as a bar under the scale) and at `/api/spots`:

```toml
[[skimmer]]
mode = "ft8"           # ft8 | ft4 | wspr
dial = 7074000.0       # USB dial, Hz
hours = "18-06"        # UTC; empty: always

[station]
callsign = "N0CALL"
locator = "FN42aa"     # or from gpsd:
gpsd = "127.0.0.1"

[report]
pskreporter = true     # FT8/FT4, 5-minute batches
wsprnet = true         # WSPR

[syslog]
server = "192.0.2.10:514"
events = ["spot", "band", "upload"]   # what goes there
modes = []                            # spots of these modes only; empty: all

[mqtt]
server = "192.0.2.10:1883"
topic = "web888"
```

Only spots from live signals are uploaded: the iq backend on a
recording and the sim backend never upload. Skimmers give their channel
to web users when channels run out (`[limits] skimmers_yield`) and take
one again later. On the A9, FT8 on eight bands keeps the two cores about
84 % busy; adding FT4 and WSPR on top of five FT8 bands makes the
decoders drop slots (`doc/m4-skimmers.md` §7). Plan and scores against
jt9 and wsprd: `doc/m4-skimmers.md`.

### Real signals without a Web-888 (HackRF, IQ recordings)

`--backend iq` feeds web888d from a wideband IQ stream instead of the
Web-888's ADC: the same channel structure as the FPGA (NCO, CIC chains,
rates, levels, overrun accounting), but only within the input's band. The
input rate must be 122.88 MHz / 8, 16, ... (7.68 MS/s: channels within
+-3.46 MHz of the centre; 15.36 MS/s covers 40 to 20 m at twice the CPU).

```
# live (needs libSoapySDR): 40 m + 30 m around 8.6 MHz
cargo run --release -p web888d --features soapy -- serve --backend iq \
  --iq soapy:driver=hackrf --iq-freq 8.6e6 --iq-bw 7e6 \
  --iq-gain LNA=16 --iq-gain VGA=20 --iq-gain AMP=0

# record, then replay (looping) without the device
hackrf_transfer -r hf.cs8 -f 8600000 -s 7680000 -b 7000000 -l 16 -g 20 -a 0 -n 230400000
cargo run --release -p web888d -- serve --backend iq --iq file:hf.cs8 --iq-freq 8.6e6 --iq-loop
```

Formats: `.cs8` (hackrf_transfer), `.cs16`, `.cf32`. On an HF antenna,
keep the HackRF's amplifier off and watch for overload (input samples at
full scale count as ADC overflows: HPSDR overload bit, `hw_stats`). About
6 % of one core per active channel at 7.68 MS/s
(`cargo run --release -p web888-iq --example bench 7`).

Without an SDR program, `p1probe` checks a running server: it tunes
receivers, streams for a few seconds and prints rate, sequence gaps and the
strongest tones per receiver:

```
cargo run --release -p web888-hpsdr --example p1probe -- UNIT:1024 --secs 5 7074000 14074000
```

The same works against the QEMU virtual Web-888 in web888-ng-hw
(`emu/`, stock firmware against an FPGA model): boot it, fetch the
binary in the guest with `wget http://10.0.2.2:PORT/web888d` from a host
HTTP server, run `web888d serve`, then probe `127.0.0.1:1024` (UDP
forwarded). Tones arrive at the model's levels and web888d reads the
full 48 kS/s.

CI builds a static `armv7-unknown-linux-musleabihf` binary for the device.

## Licence

GPL-3.0-or-later (`COPYING`). Code ported from RaspSDR/server (KiwiSDR-derived)
keeps its original notices. Provenance rules are in `doc/design.md` §12.
Not affiliated with the Web-888 / RX-888 vendor.
