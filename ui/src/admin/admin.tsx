// SPDX-License-Identifier: GPL-3.0-or-later
// The admin pages under #/admin (server/doc/m3-multi-user.md §7): status
// dashboard, sessions, configuration, passwords. They poll the admin API;
// the receiver page is separate (no WebSocket here).

import { signal } from "@preact/signals";
import { useEffect, useRef, useState } from "preact/hooks";
import { formatKhz } from "../fmt.ts";
import { call, get, loggedOut, type Auth, type LeaseRow, type Session, type Skimmers, type Stats } from "./api.ts";
import { ConfigPage } from "./configform.tsx";

type Tab = "status" | "sessions" | "config" | "passwords";
const TABS: { id: Tab; name: string }[] = [
  { id: "status", name: "Status" },
  { id: "sessions", name: "Sessions" },
  { id: "config", name: "Configuration" },
  { id: "passwords", name: "Passwords" },
];

function tabFromHash(): Tab {
  const t = location.hash.replace(/^#\/admin\/?/, "") as Tab;
  return TABS.some((x) => x.id === t) ? t : "status";
}

const tab = signal<Tab>(tabFromHash());
window.addEventListener("hashchange", () => (tab.value = tabFromHash()));

/** Call `f` now and every `ms` while mounted. */
function usePoll(f: () => Promise<void>, ms: number) {
  const ref = useRef(f);
  ref.current = f;
  useEffect(() => {
    let stop = false;
    let t: number | undefined;
    const run = async () => {
      await ref.current().catch(() => {});
      if (!stop) t = window.setTimeout(run, ms);
    };
    void run();
    return () => {
      stop = true;
      clearTimeout(t);
    };
  }, [ms]);
}

function ago(unixS: number): string {
  const s = Math.max(0, Date.now() / 1000 - unixS);
  if (s < 60) return `${s.toFixed(0)} s`;
  if (s < 3600) return `${(s / 60).toFixed(0)} min`;
  return `${Math.floor(s / 3600)} h ${Math.floor((s % 3600) / 60)} min`;
}

const khz = (hz: number | null) => (hz === null ? "–" : `${formatKhz(hz)} kHz`);

async function restart(): Promise<void> {
  if (!confirm("Restart web888d? Everyone is disconnected for a few seconds.")) return;
  await call("POST", "api/admin/restart").catch((e: Error) => alert(e.message));
  // come back when it's up again
  const t0 = Date.now();
  const wait = async () => {
    await new Promise((r) => setTimeout(r, 2000));
    try {
      await get("api/auth");
      location.reload();
    } catch {
      if (Date.now() - t0 < 60_000) await wait();
    }
  };
  await wait();
}

function Login({ auth, onDone }: { auth: Auth; onDone(): void }) {
  const [pw, setPw] = useState("");
  const [err, setErr] = useState("");
  if (!auth.admin_password) {
    return (
      <div class="admin-login">
        <h2>web888d admin</h2>
        <p>No admin password is set. On the unit's console run</p>
        <pre>web888d passwd</pre>
        <p>
          or put the password (one line, at least 8 characters) in <code>/boot/web888d-admin</code> on the SD card and restart.
        </p>
      </div>
    );
  }
  const submit = async (e: Event) => {
    e.preventDefault();
    setErr("");
    try {
      await call("POST", "api/login", { password: pw, admin: true });
      onDone();
    } catch (x) {
      setErr((x as Error).message);
    }
  };
  return (
    <form class="admin-login" onSubmit={submit}>
      <h2>web888d admin</h2>
      <input
        type="password"
        placeholder="Admin password"
        autocomplete="current-password"
        autofocus
        value={pw}
        onInput={(e) => setPw((e.target as HTMLInputElement).value)}
      />
      <button type="submit" class="on" disabled={!pw}>
        Log in
      </button>
      {err && <p class="warn">{err}</p>}
      <p>
        <a href="#">Back to the receiver</a>
      </p>
    </form>
  );
}

function StatusPage() {
  const [stats, setStats] = useState<Stats | null>(null);
  const [leases, setLeases] = useState<{ channels: number; leases: LeaseRow[] } | null>(null);
  const prev = useRef<{ t: number; s: Stats } | null>(null);
  const [rates, setRates] = useState<{ frames: number; busy: number[] } | null>(null);
  const [skim, setSkim] = useState<Skimmers | null>(null);
  usePoll(async () => {
    const [s, l, k] = await Promise.all([
      get<Stats>("api/admin/stats"),
      get<{ channels: number; leases: LeaseRow[] }>("api/admin/leases"),
      get<Skimmers>("api/admin/skimmers"),
    ]);
    setSkim(k);
    const t = performance.now();
    const p = prev.current;
    if (p) {
      const dt = (t - p.t) / 1000;
      setRates({
        frames: (s.hw.rx_frames - p.s.hw.rx_frames) / dt,
        busy: s.pool.map((w, k) => (100 * (w.busy_s - (p.s.pool[k]?.busy_s ?? 0))) / dt),
      });
    }
    prev.current = { t, s };
    setStats(s);
    setLeases(l);
  }, 2000);
  if (!stats || !leases) return <p class="dim">Loading…</p>;
  const byChannel = new Map(leases.leases.filter((l) => l.channel !== null).map((l) => [l.channel!, l]));
  const queue = leases.leases.filter((l) => l.waiting !== null).sort((a, b) => a.waiting! - b.waiting!);
  const threads = [...stats.threads].sort((a, b) => (b.cpu_pct ?? 0) - (a.cpu_pct ?? 0));
  return (
    <div class="cards">
      <section class="card">
        <h3>Radio</h3>
        <dl>
          <dt>Sessions</dt>
          <dd>{stats.sessions}</dd>
          <dt>RX rate</dt>
          <dd class={stats.hw.hw_rate_ok ? "" : "warn"}>
            {rates ? `${(rates.frames / 1e3).toFixed(1)} k frames/s` : "…"}
            {!stats.hw.hw_rate_ok && " (falling behind)"}
          </dd>
          <dt>ADC overloads</dt>
          <dd>{stats.hw.adc_overloads} blocks</dd>
          <dt>Clock</dt>
          <dd>{stats.hw.clock_ppm === 0 ? "no GPS correction" : `${stats.hw.clock_ppm.toFixed(3)} ppm`}</dd>
        </dl>
        <button onClick={() => void restart()}>Restart web888d…</button>
      </section>
      <section class="card">
        <h3>
          RX channels ({byChannel.size}/{leases.channels} in use)
        </h3>
        <table>
          <tbody>
            {Array.from({ length: leases.channels }, (_, ch) => {
              const l = byChannel.get(ch);
              return (
                <tr key={ch} class={l ? "" : "dim"}>
                  <td>{ch}</td>
                  <td>{l ? l.label : "free"}</td>
                  <td class="r">{l ? khz(l.hz) : ""}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
        {queue.length > 0 && (
          <>
            <h4>Waiting</h4>
            <ol>
              {queue.map((l) => (
                <li key={l.id}>{l.label}</li>
              ))}
            </ol>
          </>
        )}
      </section>
      <section class="card">
        <h3>DSP workers</h3>
        <table>
          <thead>
            <tr>
              <th>#</th>
              <th class="r">jobs</th>
              <th class="r">busy</th>
              <th class="r">shed</th>
              <th class="r">drops rx/wf</th>
            </tr>
          </thead>
          <tbody>
            {stats.pool.map((w, k) => (
              <tr key={k}>
                <td>{k}</td>
                <td class="r">{w.jobs}</td>
                <td class="r">{rates ? `${rates.busy[k].toFixed(1)} %` : "…"}</td>
                <td class={`r ${w.shed > 0 ? "warn" : ""}`}>{w.shed}</td>
                <td class="r">
                  {w.rx_dropped}/{w.wf_dropped}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        <h4>Waterfall views</h4>
        {stats.wf_views.length === 0 ? (
          <p class="dim">none</p>
        ) : (
          <table>
            <tbody>
              {stats.wf_views.map((v, k) => (
                <tr key={k}>
                  <td>slot {v.slot}</td>
                  <td class="r">{khz(v.hz)}</td>
                  <td class="r">÷{v.decim}</td>
                  <td class="r">{v.fps} fps</td>
                  <td class="r">
                    {v.sessions.length} session{v.sessions.length === 1 ? "" : "s"}
                    {v.shared ? ", time-shared" : ""}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>
      {skim?.available && <SkimCard skim={skim} />}
      <section class="card">
        <h3>CPU per thread</h3>
        <table>
          <thead>
            <tr>
              <th>thread</th>
              <th class="r">now</th>
              <th class="r">total</th>
            </tr>
          </thead>
          <tbody>
            {threads.map((t) => (
              <tr key={t.name}>
                <td>{t.name}</td>
                <td class="r">{t.cpu_pct === null ? "…" : `${t.cpu_pct.toFixed(1)} %`}</td>
                <td class="r">{t.cpu_s.toFixed(0)} s</td>
              </tr>
            ))}
          </tbody>
        </table>
        <p class="dim">% of one core since the last refresh, summed over threads of a name.</p>
      </section>
    </div>
  );
}

function SkimCard({ skim }: { skim: Skimmers }) {
  const bands = skim.bands ?? [];
  const p = skim.pskreporter;
  return (
    <section class="card">
      <h3>Skimmers</h3>
      {bands.length === 0 ? (
        <p class="dim">None configured ([[skimmer]] in the config).</p>
      ) : (
        <table>
          <thead>
            <tr>
              <th>band</th>
              <th>state</th>
              <th class="r">spots/slot</th>
              <th class="r">CPU/slot</th>
              <th class="r">last</th>
            </tr>
          </thead>
          <tbody>
            {bands.map((b, k) => {
              const state = !b.enabled ? "off" : !b.running ? `idle (hours ${b.hours})` : b.granted ? "running" : "waiting";
              const late = b.running && b.granted && (!b.last_slot || Date.now() / 1000 - b.last_slot > 60);
              return (
                <tr key={k} class={b.running ? "" : "dim"}>
                  <td>
                    {b.mode.toUpperCase()} {formatKhz(b.dial).slice(0, -4)} {b.band && <span class="dim">{b.band}</span>}
                  </td>
                  <td class={b.running && !b.granted ? "warn" : ""}>{state}</td>
                  <td class="r">{b.slots ? (b.spots! / b.slots).toFixed(1) : "–"}</td>
                  <td class="r">{b.slots ? `${(b.cpu_per_slot_s! * 1e3).toFixed(0)} ms` : "–"}</td>
                  <td class={`r ${late ? "warn" : ""}`}>{b.last_slot ? `${ago(b.last_slot)} ago` : "–"}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
      {bands.some((b) => (b.dropped ?? 0) + (b.cut_short ?? 0) > 0) && (
        <p class="warn">
          Behind: {bands.reduce((a, b) => a + (b.dropped ?? 0), 0)} slots dropped, {bands.reduce((a, b) => a + (b.cut_short ?? 0), 0)} cut short by
          the deadline.
        </p>
      )}
      <h4>PSKReporter</h4>
      {!p || !p.enabled ? (
        <p class="dim">Not uploading{!skim.callsign || (skim.locator ?? "").length < 4 ? ": [station] needs a callsign and a locator" : ""}.</p>
      ) : (
        <dl>
          <dt>As</dt>
          <dd>
            {skim.callsign} {skim.locator}
          </dd>
          <dt>Sent</dt>
          <dd>
            {p.reports} reports in {p.datagrams} datagrams{p.last_sent ? `, last ${ago(p.last_sent)} ago` : ""}
          </dd>
          <dt>Waiting</dt>
          <dd>{p.pending}</dd>
          <dt>Not sent</dt>
          <dd>
            {p.skipped_recent} reported within the hour, {p.skipped_not_live} not live
          </dd>
          {p.last_error && (
            <>
              <dt>Error</dt>
              <dd class="warn">{p.last_error}</dd>
            </>
          )}
        </dl>
      )}
      {!skim.live && <p class="warn">This radio isn't live (a recording or a simulation): no spots are uploaded.</p>}
    </section>
  );
}

function SessionsPage() {
  const [list, setList] = useState<Session[] | null>(null);
  const [err, setErr] = useState("");
  const refresh = async () => {
    const r = await get<{ sessions: Session[] }>("api/admin/sessions");
    setList(r.sessions);
  };
  usePoll(refresh, 2000);
  const kick = async (s: Session) => {
    if (!confirm(`Disconnect session ${s.id} (${s.ip})?`)) return;
    setErr("");
    await call("POST", `api/admin/sessions/${s.id}/kick`).catch((e: Error) => setErr(e.message));
    setTimeout(() => void refresh(), 300);
  };
  if (!list) return <p class="dim">Loading…</p>;
  return (
    <div>
      {err && <p class="warn">{err}</p>}
      {list.length === 0 ? (
        <p class="dim">No web sessions.</p>
      ) : (
        <table class="wide">
          <thead>
            <tr>
              <th>#</th>
              <th>address</th>
              <th>class</th>
              <th>for</th>
              <th class="r">frequency</th>
              <th>mode</th>
              <th>receiver</th>
              <th>shared</th>
              <th class="r">drops a/wf</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {list.map((s) => (
              <tr key={s.id}>
                <td>{s.id}</td>
                <td>{s.ip}</td>
                <td>{s.class}</td>
                <td>{ago(s.since)}</td>
                <td class="r">{khz(s.hz)}</td>
                <td>{s.mode ?? "–"}</td>
                <td>
                  {s.channel !== null ? `channel ${s.channel}` : s.waiting !== null ? `#${s.waiting} in queue` : "–"}
                  {s.listen !== null && `, listening to #${s.listen}`}
                </td>
                <td>{s.share ? "yes" : "no"}</td>
                <td class="r">
                  {s.audio_drops}/{s.wf_drops}
                </td>
                <td>
                  <button onClick={() => void kick(s)}>Kick</button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

function PasswordsPage() {
  const [auth, setAuth] = useState<Auth | null>(null);
  const [a1, setA1] = useState("");
  const [a2, setA2] = useState("");
  const [l1, setL1] = useState("");
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const load = () => get<Auth>("api/auth").then(setAuth);
  useEffect(() => void load(), []);
  const send = async (role: "admin" | "listener", password: string | null, done: string) => {
    setMsg(null);
    try {
      await call("POST", "api/admin/password", { role, password });
      setMsg({ ok: true, text: done });
      setA1("");
      setA2("");
      setL1("");
      await load();
    } catch (e) {
      setMsg({ ok: false, text: (e as Error).message });
    }
  };
  const val = (f: (v: string) => void) => (e: Event) => f((e.target as HTMLInputElement).value);
  return (
    <div class="cards">
      <section class="card">
        <h3>Admin password</h3>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void send("admin", a1, "Admin password changed; other admin logins have ended.");
          }}
        >
          <input type="password" placeholder="New password" autocomplete="new-password" value={a1} onInput={val(setA1)} />
          <input type="password" placeholder="Again" autocomplete="new-password" value={a2} onInput={val(setA2)} />
          <button type="submit" disabled={a1.length < 8 || a1 !== a2}>
            Change
          </button>
          <p class="dim">At least 8 characters.{a2 && a1 !== a2 ? " The two differ." : ""}</p>
        </form>
      </section>
      <section class="card">
        <h3>Listener password</h3>
        <p>
          {auth?.listener_password
            ? "Set: listeners need it (or the admin password) to use the receiver."
            : "Not set: anyone who reaches the page can listen."}
        </p>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void send("listener", l1, "Listener password set; listeners logged in with the old one must log in again.");
          }}
        >
          <input type="password" placeholder="New listener password" autocomplete="new-password" value={l1} onInput={val(setL1)} />
          <button type="submit" disabled={!l1}>
            {auth?.listener_password ? "Change" : "Set"}
          </button>
          {auth?.listener_password && (
            <button type="button" onClick={() => void send("listener", null, "Listener password removed: the receiver is open.")}>
              Remove
            </button>
          )}
        </form>
      </section>
      {msg && <p class={msg.ok ? "ok" : "warn"}>{msg.text}</p>}
    </div>
  );
}

export function AdminApp() {
  const [auth, setAuth] = useState<Auth | null>(null);
  const [err, setErr] = useState("");
  const refresh = () => {
    loggedOut.value = false;
    get<Auth>("api/auth")
      .then(setAuth)
      .catch((e: Error) => setErr(e.message));
  };
  useEffect(refresh, []);
  // a refused call: the login ended
  useEffect(() => {
    if (loggedOut.value) refresh();
  }, [loggedOut.value]);
  if (err) return <p class="warn admin-login">{err}</p>;
  if (!auth) return <p class="dim admin-login">Loading…</p>;
  if (auth.role !== "admin") return <Login auth={auth} onDone={refresh} />;
  const logout = async () => {
    await call("POST", "api/logout").catch(() => {});
    refresh();
  };
  return (
    <div class="admin">
      <header>
        <strong>web888d admin</strong>
        <nav class="seg">
          {TABS.map((t) => (
            <button key={t.id} class={tab.value === t.id ? "on" : ""} onClick={() => (location.hash = `#/admin/${t.id}`)}>
              {t.name}
            </button>
          ))}
        </nav>
        <span class="spacer" />
        <a href="#">Receiver</a>
        <button onClick={() => void logout()}>Log out</button>
      </header>
      <div class="page">
        {tab.value === "status" && <StatusPage />}
        {tab.value === "sessions" && <SessionsPage />}
        {tab.value === "config" && <ConfigPage onRestart={() => void restart()} />}
        {tab.value === "passwords" && <PasswordsPage />}
      </div>
    </div>
  );
}
