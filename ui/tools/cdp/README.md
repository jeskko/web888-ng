# UI checks in headless Chromium

Scripts that drive the web UI of a running web888d over the DevTools
protocol and print what they see (plus screenshots). They are manual
checks for UI work, not CI tests.

* Chromium: Playwright's cached build works:
  `CHROME=~/.cache/ms-playwright/chromium-*/chrome-linux64/chrome`.
* Always muted (`lib.mjs` passes `--mute-audio`), so nothing plays on
  the machine's speakers.
* `CDP_PORT` (default 9333) if the default DevTools port is taken.
* `CHROME_ARGS`: more Chromium arguments, separated by `|`. To see the
  page as over plain HTTP from another machine (no secure context: the
  audio fallback), map a made-up name to the local server:
  `CHROME_ARGS='--host-resolver-rules=MAP web888.test 127.0.0.1'` and open
  `http://web888.test:28175/`.

Run a throwaway server on its own port with the simulated radio. Stop it
by the PID you started, never by name: other web888d or browser
processes on the machine may not be yours.

```sh
./target/release/web888d serve --config /tmp/t/c.toml --backend sim --noise \
  --no-hpsdr --no-tls --http 127.0.0.1:28175 --state-dir /tmp/t --ui-dir ui/dist &
P=$!
CHROME=... node ui/tools/cdp/receiver.mjs http://127.0.0.1:28175/
kill $P
```

| script | checks | server needs |
|---|---|---|
| `receiver.mjs` | audio start, zoom, click-tune, mode key; `W`/`H`/`DPR` | — |
| `soak.mjs` | status bar every minute while audio plays | — |
| `shot.mjs` | one screenshot at a size, canvas sizes, horizontal scroll | — |
| `login.mjs` | listener-password form, wrong then right | `web888d passwd --listener`; `LISTENER_PW` |
| `admin.mjs` | admin pages, config form, phone layout | an admin password; `ADMIN_PW` |
| `phone.mjs` | the phone layout's panels with touch: More, Spots, Map (tap, tap again to tune), FT8/FT4/WSPR, Labels, status | a running server; `OUT` dir as the 2nd argument |
| `ops.mjs` | Logs (each source, the filter), Configuration's Backup (download, then restore it through the file input), Update's Boot loader card, the phone layout | an admin password; `ADMIN_PW`, `OUT` |
| `update.mjs` | the Update tab: look for an OS release; `ACT=install` installs it into the other slot, `ACT=reboot` reboots | the OS image with RAUC (the emulator); `ADMIN_PW` |
| `idle.mjs` | idle limit ends the session, no auto-reconnect | `[limits] idle_minutes = 1` |
| `features.mjs` | name, users list, hover preview, tuning step, AM/SAM/SAL/SAR/SAS cycle, CPU/temperature | — (the iq backend on a recording shows AM stations) |
| `spots.mjs` | spot list, band filter, row click tunes, labels on the scale | `[[skimmer]]` bands with spots (live, or the iq backend on a recording) |
| `map.mjs` | the map: stations shown, the three projections, drag, zoom, hover tooltip (screenshots) | `[dxspots]` (internet) or skimmer spots; `[station] locator` |
| `dxspots.mjs` | DX spot labels near 40 m, a typed frequency in view (the view stays) and off it (the view follows) | `[dxspots]` on (the default) and internet |
| `digi.mjs` | FT8/FT4/WSPR decoding in the page: mode, dial, Deep, the status line and the rows | real signals: live, or a recording replayed from the same second of the slot as it was recorded (`2026-10-22_..._60s.cs8` started 8.7 s into a 15 s slot) |
| `cw.mjs` | CW decoding in the page: the signals' rows (frequency, WPM, dB, text) every 15 s; tune with the URL's hash (`#f=7018.0&m=usb`), a passband button label as the 2nd argument; `NEURAL=0/1`, `WIDE=1` (skim the bin: bins mode) | real CW (live or a recording, e.g. 40 m in `2026-10-22_..._60s.cs8`) |
| `rtty.mjs` | FSK/RTTY decoding in the page: picks a station from the panel's list (centre Hz as the 2nd argument), prints each signal's parameters and text every 15 s; `WIDE=1` skims the bin | real FSK (DWD 10100.8 kHz is in `2026-10-22_..._60s.cs8`) |
| `psk.mjs` | PSK31/63/125 decoding in the page: picks a band from the panel's list (USB dial Hz as the 2nd argument), prints each signal's rate, quality and text every 15 s | real PSK (none on the corpus recordings; on air, e.g. 14070 kHz) |
| `hfdl.mjs` | HFDL decoding in the page: picks a channel from the panel's list (listed frequency Hz as the 2nd argument), prints the decoded bursts every 15 s | HFDL: a channel on air (e.g. 8942 kHz, Shannon), or a recording (`--backend iq`) |
| `img.mjs` | Fax/SSTV decoding in the page: picks a station (`f<carrier Hz>` fax, `s<dial Hz>` SSTV), `start` as the 4th argument starts a fax by hand, prints the status every 15 s, saves img.png | real fax or SSTV (DWD 7880 kHz is in `2026-10-22_..._60s.cs8`, no start tone) |
| `sigid.mjs` | signal identification in the page: the ID panel's rows (frequency, level, what it looks like) every 15 s; tune with the URL's hash, a passband button label as the 2nd argument | real signals (live, or a recording: FT8, RTTY, fax, SSB and AM are in `2026-10-22_..._60s.cs8`) |

Set passwords in the throwaway config with
`echo PASSWORD | web888d passwd --config /tmp/t/c.toml` (and
`--listener`).
