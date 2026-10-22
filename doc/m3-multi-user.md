# M3 multi-user: implementation plan

Status: **agreed** (2026-10-22; decisions in §10). Scope from `design.md` §11 M3:
multi-user leases, waterfall scheduler, AGC/NB/NR, admin API, auth.
Done when (design.md): 13 users at once with time-shared waterfalls.
Builds on M2 (`m2-web-ui.md`); decisions there stand.

Concretely, done when:

* 12 web users (HF: 12 RX channels) listen at the same time, each with
  their own waterfall view, and a 13th gets a waterfall-only session
  (§2.3). Checked with a load tool (§8) against the sim and iq backends,
  and on the test unit once it is here (CPU numbers only from the board).
* HPSDR clients and web users share the channels through the same lease
  manager.
* An admin logs in, sees sessions/leases/CPU/drops, kicks a user, edits
  the config (persisted, §6), and sets limits.

---

## 1. What changes, in one picture

```
            ┌──────────────── channel manager (leases) ───────────────┐
 sessions ─▶│ RX channel pool (12/13)   WF scheduler (2 hw slots)      │──HwCmd──▶ hw thread
 (web, HPSDR│ priorities, limits         shared views, time-sharing    │
  later     └──────────────────────────────────────────────────────────┘
  decoders)          │ grants/revokes                │ WF blocks
                     ▼                               ▼
              DSP pool (N = cores) ── per-session audio jobs, per-view WF jobs
                     │
              tokio: WS sessions, admin API, /metrics, auth
```

M2 has one fixed channel and one exclusive WF slot for one user, two
threads per session. M3 replaces that with a **channel manager** that owns
all hardware assignments, a **WF scheduler**, and a **DSP pool**.

## 2. Channels and leases (`crates/core`, new)

### 2.1 Model

* **Session**: a client with an identity (web user, HPSDR client, later a
  decoder), a class and limits.
* **Lease**: a session's claim on one hardware RX channel. Classes, highest
  first: `admin > skimmer > protocol > web` (design.md §6). Skimmers
  arrive with M4; the class exists now.
* The **channel manager** is the only code that retunes RX NCOs. Sessions
  ask it (`Acquire`, `Tune`, `Release`); it answers with grants and
  revocations. It runs as a task on the tokio runtime, and talks to the hw
  thread with `HwCmd` (as now).
* HPSDR: receivers become leases, taken when a client starts streaming and
  released when it stops. Today it holds channels `0..7` for its lifetime.

### 2.2 Policies (config, §6)

* Per-IP session limit (default 4), per-class channel quotas (e.g.
  HPSDR at most 7), optional time limit per web session (default none),
  idle timeout (no WS traffic / tab hidden: the UI says so; default
  none on LAN).
* Pre-emption: a higher class takes a channel from a lower one only if
  configured (`preempt = true`, default **off**); the evicted session
  falls back to waterfall-only (§2.3), not disconnected.

### 2.3 When the channels run out

The 13th web user gets a **waterfall-only session** with a "waiting for a
receiver" notice. It is queued and gets audio as soon as a channel frees
(FIFO within its class).

While waiting, it can **listen along** to another user's channel (any user
who hasn't opted out, §8): its own audio chain (mode, passband, AGC,
NB/NR, volume) runs on that channel's IQ, and the dial follows the
channel's owner. This costs DSP, no hardware channel. When its own
receiver is granted, the UI offers to switch. The opt-out is one per-user
setting covering both the frequency marker and listen-along (default:
shared); an admin can set the default and force it off per receiver.

### 2.4 Not in M3: sharing one hardware channel

Two users tuned within ±20 kHz of each other could share one 48 kS/s
channel (a software shift for the second). That multiplies capacity in
crowded bands, but it couples their tuning and complicates the manager.
It's left as a later optimisation, with the lease API shaped so it can be
added (a lease is "a channel + an offset").

## 3. Waterfall scheduler

Requests: one per session, (centre, decimation, bins, fps). Hardware: 2 WF
slots. Three mechanisms, applied in order:

1. **Shared views.** Sessions with the same (centre, decimation) share one
   stream and one FFT engine (`WfEngine` per view, lines fanned out to
   its sessions, re-binned per session's `bins`). The default full view is
   the common case, so most new users cost nothing extra.
2. **Narrow views from the user's own RX channel.** A view whose span is
   ≤ 40 kHz (inside the ±24 kHz of a 48 kS/s channel) and contains the
   user's audio channel is computed from that channel's IQ, so it needs no
   WF slot. This covers "zoomed in on the signal I'm listening to".
3. **Time-sharing.** With more distinct views than slots, the scheduler
   gives each view slot time in turn: one-shot 8192-sample blocks
   (exactly how the stock driver works anyway), weighted by requested fps
   and by span (narrow spans take longer to fill a block). With ≤ 2
   distinct views the slots stay continuous (exclusive mode, as in M2).
   The Welch engine already copes with sparse blocks: a line averages
   whatever blocks arrived in its interval, and repeats the newest when
   none did.

Fairness metric per view: achieved fps vs requested, shown in the admin
page. Unit tests run the scheduler against a fake clock and a fake radio
(block arrival times by span).

## 4. DSP pool

Two threads per session (M2) is 26 threads for 13 users on 2 cores. M3
uses a pool of `N = cores` workers (2 on the Zynq), each pinned:

* jobs: "process this RxBlock for these audio sessions" (split across
  workers by session), and "push this WfBlock into this view's engine"
* per-session state (`AudioChain`) is owned by one worker at a time
  (sessions are assigned round-robin; rebalanced on join/leave)
* the hw thread keeps one fan-out to the pool, not one per session
* load shedding when behind: WF budget per view drops first, then fps;
  audio is never dropped deliberately (design.md §5.2, "frame rate is
  adaptive under load")

Budget on the A9 is an estimate until the test unit arrives: the audio
chain is about 2 M multiply-adds/s per user at 48 kS/s → 12 kS/s (160-tap
decimator + ~300-tap filter at 12 k via overlap-save). 12 users ≈ 25 M/s,
well inside two A9 cores; the WF FFTs dominate and are what the budget and
shared views control. Benchmarks (`cargo bench`-style examples) for the
chain and the engine get armv7 builds so they run on the board day one.

## 5. DSP additions

* **AGC**: AM switches to a carrier-referenced AGC (gain from the
  carrier level, so modulation depth is kept); SSB/CW get a threshold
  ("AGC knee") and a fast/slow choice.
* **Noise blanker** (NB): impulse blanker on the channel's IQ before the
  channel filter (at 48 k, where pulses are still short): blank samples
  above k × the running mean magnitude, with a short taper.
* **Noise reduction** (NR): spectral subtraction on the audio (overlap-add
  STFT at 12 k, noise estimate by minimum statistics), strength 0..1.
* **Squelch** for AM/CW (RSSI threshold with hysteresis).
* Not M3: SAM, NFM, notch filters (candidates for M3.5 if wanted).

Each block gets unit tests in `crates/dsp` like the M2 ones (e.g. NB: a
-60 dBFS tone with 100 Hz impulses at +40 dB, tone SNR restored within
3 dB of the impulse-free case).

## 6. Configuration

* TOML file (`--config`, default `/etc/web888d/web888d.toml`); every CLI
  option has a config key, the CLI overrides the file.
* Hot reload of non-hardware settings (limits, auth, UI defaults) on
  change or on SIGHUP; hardware settings (backend, bitstream, rates) need
  a restart and say so.
* Admin edits go through the API, are validated, and are written
  atomically (temp file + rename) to the config path.
* **Where it persists on the device**: the root is read-only, and the
  config partition from `docs/os-base.md` doesn't exist yet. See question 5.

## 7. Auth and admin

* **Admin**: one admin account, password hashed with argon2 (crate
  `argon2`, MIT/Apache) in the config. Set at first start from the
  console (`web888d passwd`), or by the first login on a fresh unit if
  it's reachable only from the LAN (question 3).
* Login: `POST /api/login` → session cookie (HttpOnly, Secure, SameSite=
  Strict, random 256-bit id, server-side session store, 12 h expiry).
  Failed logins rate-limited per IP. All admin endpoints check it; the
  WebSocket gets the admin class when the cookie is present.
* **Listeners**: anonymous by default; an optional listener password
  (shared) for private receivers (question 2).
* **Admin API** (JSON, `/api/admin/...`): sessions (list, kick), leases,
  channel reservations, config (get, validate, put), stats (per-thread CPU
  from `/proc/self/task`, queue depths, drops, WF fairness), restart.
* **Metrics**: `GET /metrics` (Prometheus text), off by default.
* **Admin UI**: Preact pages in the same SPA under `#/admin` (status
  dashboard, sessions, config form generated from a schema the server
  serves, so the form follows the config). The receiver page stays as is.

## 8. Protocol v2 and the load tool

* `hello` gains the session id, class, and whether the session has a
  receiver (`audio: true/false`); a `lease` message reports grants and
  losses ("waiting for a receiver", "receiver granted").
* `users` message (1 Hz): other users' dial frequencies and modes (those
  who haven't opted out), drawn as markers on the frequency scale
  (Kiwi-style); a queued user picks one to listen along (§2.3).
* `share` (client): the user's opt-out; `listen` (client): listen along
  to a user id, or stop.
* New controls: `nb`, `nr`, `squelch`, `agc` threshold/speed.
* Versioning: the server accepts v1 clients until the UI ships v2 (same
  release), then v1 is dropped.
* **Load tool** (`crates/web/examples/webload.rs`): opens N WebSocket
  sessions with given tunings/views, counts audio frames, gaps and WF
  fps per session, prints a table. Runs in CI against the sim (N = 13),
  and against the board later. No browser needed.

## 9. Steps (each ends in a commit)

| # | content | done when |
|---|---|---|
| 1 | config file + CLI merge, hot reload | config round-trip tests; serve reads it |
| 2 | channel manager + leases; HPSDR on leases | unit tests (grant, release, quotas, queue, pre-emption off/on) |
| 3 | DSP pool; sessions use it (audio) | M2 integration test passes on the pool; 13-session sim test |
| 4 | WF scheduler: shared views, RX-channel views, time-sharing | scheduler tests with a fake clock; load tool shows fair fps |
| 5 | protocol v2, waterfall-only sessions, queue, listen-along, UI for lease state + user markers | load tool: 12 audio + 1 queued listening along, which gets its own receiver when one leaves; opted-out users can't be followed |
| 6 | DSP: carrier AGC, NB, NR, squelch; UI controls | dsp unit tests; listening check (user) |
| 7 | auth, admin API, `/metrics` | API tests incl. auth failures and rate limit |
| 8 | admin UI (Preact): dashboard, sessions, config form | browser check (muted) |
| 9 | armv7 benches, docs, release v0.2.0 (pre-release, with go-ahead) | benches run on the test unit when it's here |

## 10. Decisions (reviewed 2026-10-22)

1. **Channels exhausted**: queue with a waterfall-only session, and
   listen-along to other users' channels while waiting (§2.3; user's
   addition). Pre-emption by class stays an off-by-default option.
2. **Listener access**: anonymous by default, optional shared listener
   password; per-user accounts later.
3. **First admin password**: set from the console (`web888d passwd`) or
   `/boot/web888d-admin` read once at first boot and deleted; never a
   default password.
4. **Other users' frequencies** on the scale: yes, with a per-user
   opt-out (also covers listen-along) and an admin override per receiver.
5. **Config persistence**: the os image gets the small config partition
   (ext4, written only on save, `docs/os-base.md`) in step 1; config and
   the self-signed certificate live there.
6. **Hardware RX rate**: stays 48 kS/s; the 24 k idea is dropped unless
   board CPU numbers say otherwise.
