# UI checks in headless Chromium

Scripts that drive the web UI of a running web888d over the DevTools
protocol and print what they see (plus screenshots). They are manual
checks for UI work, not CI tests.

* Chromium: Playwright's cached build works:
  `CHROME=~/.cache/ms-playwright/chromium-*/chrome-linux64/chrome`.
* Always muted (`lib.mjs` passes `--mute-audio`), so nothing plays on
  the machine's speakers.
* `CDP_PORT` (default 9333) if the default DevTools port is taken.

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
| `idle.mjs` | idle limit ends the session, no auto-reconnect | `[limits] idle_minutes = 1` |

Set passwords in the throwaway config with
`echo PASSWORD | web888d passwd --config /tmp/t/c.toml` (and
`--listener`).
