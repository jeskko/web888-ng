// SPDX-License-Identifier: GPL-3.0-or-later
// The admin pages under #/admin (server/doc/m3-multi-user.md §7): status
// dashboard, sessions, configuration, passwords. They poll the admin API;
// the receiver page is separate (no WebSocket here).

import { signal } from "@preact/signals";
import { useEffect, useRef, useState } from "preact/hooks";
import { formatKhz } from "../fmt.ts";
import { call, get, href, loggedOut, putText, type Auth, type Changes, type LeaseRow, type Session, type Skimmers, type Stats, type UpdateView } from "./api.ts";
import { ConfigPage } from "./configform.tsx";

type Tab = "status" | "sessions" | "config" | "labels" | "info" | "passwords" | "update" | "logs";
const TABS: { id: Tab; name: string }[] = [
  { id: "status", name: "Status" },
  { id: "sessions", name: "Sessions" },
  { id: "config", name: "Configuration" },
  { id: "labels", name: "Labels" },
  { id: "info", name: "Info page" },
  { id: "passwords", name: "Passwords" },
  { id: "update", name: "Update" },
  { id: "logs", name: "Logs" },
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
  const [rates, setRates] = useState<{ frames: number; busy: number[]; overloads: number } | null>(null);
  const [skim, setSkim] = useState<Skimmers | null>(null);
  const [ovl, setOvl] = useState<Ovl | null>(null);
  usePoll(async () => {
    const [s, l, k, o] = await Promise.all([
      get<Stats>("api/admin/stats"),
      get<{ channels: number; leases: LeaseRow[] }>("api/admin/leases"),
      get<Skimmers>("api/admin/skimmers"),
      get<Ovl>("api/admin/ovl").catch(() => null),
    ]);
    setSkim(k);
    setOvl(o);
    const t = performance.now();
    const p = prev.current;
    if (p) {
      const dt = (t - p.t) / 1000;
      setRates({
        frames: (s.hw.rx_frames - p.s.hw.rx_frames) / dt,
        overloads: (s.hw.adc_overloads - p.s.hw.adc_overloads) / dt,
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
          <dt>Frames lost</dt>
          <dd class={stats.hw.rx_lost_frames ? "warn" : ""} title="RX frames lost to ring overruns: the reader fell behind, a gap in every listener's audio">
            {stats.hw.rx_lost_frames}
          </dd>
          <dt>ADC overloads</dt>
          <dd class={rates && rates.overloads > 0 ? "warn" : ""} title="RX blocks read while the ADC clipped: strong signals distort every listener's audio. The RF attenuator helps.">
            {stats.hw.adc_overloads} blocks{rates && rates.overloads > 0 ? ` (${rates.overloads.toFixed(1)}/s now)` : ""}
          </dd>
          <dt>RF attenuator</dt>
          <dd>
            <AttenCtl set={stats.hw.atten_db} cfg={stats.hw.atten_cfg_db} />
          </dd>
          {stats.hw.adc_dither !== undefined && (
            <>
              <dt>ADC</dt>
              <dd>
                <AdcCtl dither={!!stats.hw.adc_dither} pga={!!stats.hw.adc_pga} />
              </dd>
            </>
          )}
          <dt>Clock</dt>
          <dd>{stats.hw.clock_ppm === 0 ? "no GPS correction" : `${stats.hw.clock_ppm.toFixed(3)} ppm`}</dd>
        </dl>
        <button onClick={() => void restart()}>Restart web888d…</button>
      </section>
      {ovl && <OvlCard o={ovl} />}
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

const ATTEN_STEPS = [3, 6, 10, 15, 20, 25, 31.5];
/** The level the attenuator goes back in at. */
const attenLevel = { v: 10 };

/** The RF attenuator: in or out, and its level. It acts on every
 * listener's signals at once, so it's here and not on the receiver page;
 * saved to the configuration (`radio.atten_db`). */
function AttenCtl({ set, cfg }: { set: number; cfg: number }) {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  if (cfg > 0) attenLevel.v = cfg;
  const put = async (db: number) => {
    setBusy(true);
    setErr(null);
    try {
      await call("POST", "api/admin/atten", { db });
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  const on = cfg > 0;
  return (
    <>
      <label title="Weakens every signal before the ADC, for all listeners: against overloads from strong signals">
        <input type="checkbox" checked={on} disabled={busy} onChange={() => void put(on ? 0 : attenLevel.v)} /> in
      </label>{" "}
      <select
        value={String(attenLevel.v)}
        disabled={busy}
        onChange={(e) => {
          attenLevel.v = Number(e.currentTarget.value);
          if (on) void put(attenLevel.v);
        }}
      >
        {(ATTEN_STEPS.includes(attenLevel.v) ? ATTEN_STEPS : [...ATTEN_STEPS, attenLevel.v].sort((a, b) => a - b)).map((d) => (
          <option key={d} value={String(d)}>
            {d} dB
          </option>
        ))}
      </select>
      {set !== cfg && <span class="warn"> now {set} dB (set by an HPSDR client)</span>}
      {err && <span class="warn"> {err}</span>}
    </>
  );
}

/** The ADC's dither and PGA, for every listener at once; saved to the
 * configuration (`radio.adc_dither`, `radio.adc_pga`). */
function AdcCtl({ dither, pga }: { dither: boolean; pga: boolean }) {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const put = async (b: { dither?: boolean; pga?: boolean }) => {
    setBusy(true);
    setErr(null);
    try {
      await call("POST", "api/admin/adc", b);
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      <label title="The ADC's dither: spreads its own spurs (birdies) into the noise, at a little more noise">
        <input type="checkbox" checked={dither} disabled={busy} onChange={() => void put({ dither: !dither })} /> dither
      </label>{" "}
      <label title="The ADC's PGA: 1.5 V full scale instead of 2.25 V, every signal ~3.5 dB higher (and overloads that much sooner); the dBFS levels move with it">
        <input type="checkbox" checked={pga} disabled={busy} onChange={() => void put({ pga: !pga })} /> PGA (+3.5 dB)
      </label>
      {err && <span class="warn"> {err}</span>}
    </>
  );
}

interface OvlBin {
  bin: number;
  hz: number;
  peak_hz: number;
  dbfs: number;
  rise_db: number;
  share: number;
}
interface OvlEvent {
  start: number;
  seconds: number;
  blocks: number;
  clocks: number;
  peak_dbfs: number | null;
  analysis: { kind: "carrier" | "several" | "broadband" | "spread" | "unknown"; strongest: OvlBin[]; median_rise_db: number; total_dbfs: number };
  carrier: { hz: number; dbfs: number; snr_db: number } | null;
}
interface Ovl {
  spectrum: boolean;
  blocks: number;
  clocks: number;
  peak_dbfs: number | null;
  events: OvlEvent[];
}

const KIND: Record<OvlEvent["analysis"]["kind"], string> = {
  carrier: "a carrier",
  several: "several signals",
  broadband: "broadband (an impulse)",
  spread: "no single cause",
  unknown: "–",
};

/** What an overload event's cause was, in words. */
export function ovlCause(e: OvlEvent): string {
  const a = e.analysis;
  const mhz = (hz: number) => `${(hz / 1e6).toFixed(3)} MHz`;
  if (e.carrier) return `carrier at ${(e.carrier.hz / 1e3).toFixed(3)} kHz, ${e.carrier.dbfs.toFixed(0)} dBFS`;
  const b = a.strongest[0];
  if (a.kind === "carrier" && b) return `carrier near ${mhz(b.peak_hz)}, ${b.dbfs.toFixed(0)} dBFS (${Math.round(b.share * 100)} %)`;
  if (a.kind === "several") return `several: ${a.strongest.map((s) => mhz(s.peak_hz)).join(", ")}`;
  if (a.kind === "broadband") return `broadband, +${a.median_rise_db.toFixed(0)} dB everywhere`;
  if (a.kind === "spread" && b) return `no single cause; strongest ${mhz(b.peak_hz)} (${Math.round(b.share * 100)} %)`;
  return KIND[a.kind];
}

/** The ADC overload events (`/api/admin/ovl`), newest first. */
function OvlCard({ o }: { o: Ovl }) {
  const t = (s: number) => new Date(s * 1000).toISOString().slice(5, 19).replace("T", " ");
  return (
    <section class="card">
      <h3>ADC overloads</h3>
      <p class="dim">
        {o.events.length === 0 ? "None since web888d started." : `The last ${o.events.length} (UTC).`} Peak sample now: {o.peak_dbfs === null ? "unknown" : `${o.peak_dbfs.toFixed(1)} dBFS`}.
        {!o.spectrum && " No channelizer: the causes aren't analysed."}
      </p>
      {o.events.length > 0 && (
        <table class="ovl-events">
          <thead>
            <tr>
              <th>When</th>
              <th>Length</th>
              <th title="ADC clocks with the overflow pin high">Clocks</th>
              <th>Cause</th>
            </tr>
          </thead>
          <tbody>
            {o.events.slice(0, 30).map((e) => (
              <tr key={e.start}>
                <td>{t(e.start)}</td>
                <td>{e.seconds.toFixed(1)} s</td>
                <td>{e.clocks}</td>
                <td>{ovlCause(e)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
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
      {skim.gpsd && (
        <>
          <h4>gpsd</h4>
          <dl>
            <dt>Server</dt>
            <dd class={skim.gpsd.connected ? "" : "warn"}>
              {skim.gpsd.server} {skim.gpsd.connected ? "" : `(not connected${skim.gpsd.last_error ? `: ${skim.gpsd.last_error}` : ""})`}
            </dd>
            <dt>Fix</dt>
            <dd>
              {skim.gpsd.mode >= 3 ? "3D" : skim.gpsd.mode === 2 ? "2D" : "none"}
              {skim.gpsd.last_fix ? `, last ${ago(skim.gpsd.last_fix)} ago` : ""}
            </dd>
            <dt>Locator</dt>
            <dd>{skim.gpsd.locator ?? `none yet (using ${skim.locator || "no locator"})`}</dd>
          </dl>
        </>
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
            {p.skipped_recent} reported within the hour, {p.skipped_unconfirmed ?? 0} uncertain and not heard again, {p.skipped_not_live} not live
          </dd>
          {p.last_error && (
            <>
              <dt>Error</dt>
              <dd class="warn">{p.last_error}</dd>
            </>
          )}
        </dl>
      )}
      <h4>WSPRnet</h4>
      {!skim.wsprnet || !skim.wsprnet.enabled ? (
        <p class="dim">Not uploading ([report] wsprnet).</p>
      ) : (
        <dl>
          <dt>Sent</dt>
          <dd>
            {skim.wsprnet.sent} spots{skim.wsprnet.last_sent ? `, last ${ago(skim.wsprnet.last_sent)} ago` : ""}
          </dd>
          <dt>Waiting</dt>
          <dd>{skim.wsprnet.pending}</dd>
          {skim.wsprnet.last_reply && (
            <>
              <dt>Reply</dt>
              <dd class="dim">{skim.wsprnet.last_reply}</dd>
            </>
          )}
          {(skim.wsprnet.failed > 0 || skim.wsprnet.last_error) && (
            <>
              <dt>Failed</dt>
              <dd class="warn">
                {skim.wsprnet.failed}
                {skim.wsprnet.last_error ? ` (${skim.wsprnet.last_error})` : ""}
              </dd>
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
  const ban = async (s: Session) => {
    if (!confirm(`Ban ${s.ip}? Its sessions end now and new ones are refused (Configuration: limits.blocked lifts it).`)) return;
    setErr("");
    await call("POST", `api/admin/sessions/${s.id}/ban`).catch((e: Error) => setErr(e.message));
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
                  <button onClick={() => void kick(s)}>Kick</button>{" "}
                  {s.class !== "admin" && (
                    <button onClick={() => void ban(s)} title="End its sessions and refuse its address from now on">
                      Ban
                    </button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

interface LabelInfo {
  region: number;
  eibi_season: string;
  sources: { id: string; count: number }[];
}

/** The station's own label list: download, upload (checked by the server
 * before it is saved), and what the bundled lists hold. */
function LabelsPage() {
  const [info, setInfo] = useState<LabelInfo | null>(null);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const load = () => get<LabelInfo>("api/markers/info").then(setInfo);
  useEffect(() => void load(), []);
  const count = (id: string) => info?.sources.find((s) => s.id === id)?.count ?? 0;
  const upload = async (e: Event) => {
    const input = e.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    input.value = "";
    if (!file) return;
    setMsg(null);
    try {
      const r = await putText<{ count: number }>("api/admin/markers", await file.text(), "text/csv");
      setMsg({ ok: true, text: `Saved: ${r.count} labels from ${file.name}.` });
      await load();
    } catch (err) {
      setMsg({ ok: false, text: `${file.name}: ${(err as Error).message}. The list was not changed.` });
    }
  };
  return (
    <div class="cards">
      <section class="card">
        <h3>The station's labels</h3>
        <p>{count("own")} labels. Listeners see them in yellow (Labels: Station).</p>
        <p>
          <a href={href("api/admin/markers")} download="markers.csv">
            Download markers.csv
          </a>
          {" · "}
          <label class="file">
            Upload a new list <input type="file" accept=".csv,text/csv,text/plain" onChange={(e) => void upload(e)} />
          </label>
        </p>
        {msg && <p class={msg.ok ? "ok" : "warn"}>{msg.text}</p>}
        <p class="dim">
          A CSV file, as a spreadsheet saves it (comma or semicolon). The first line names the columns: <code>khz</code> and <code>name</code>{" "}
          are needed; <code>mode</code> (usb, lsb, cw or am; default am), <code>note</code>, <code>category</code>, <code>emission</code>,{" "}
          <code>schedule</code> and <code>dial_khz</code> may follow. An upload replaces the whole list.
        </p>
      </section>
      <section class="card">
        <h3>Bundled lists</h3>
        <table>
          <tbody>
            <tr>
              <td>Broadcast (EiBi {info?.eibi_season})</td>
              <td class="num">{count("eibi")}</td>
            </tr>
            <tr>
              <td>Utility</td>
              <td class="num">{count("utility")}</td>
            </tr>
            <tr>
              <td>Amateur</td>
              <td class="num">{count("amateur")}</td>
            </tr>
          </tbody>
        </table>
        <p class="dim">
          Listeners choose what they see. The band ribbon and the amateur frequencies follow IARU region {info?.region ?? "?"} (Configuration:
          markers.region) until a listener picks another.
        </p>
      </section>
    </div>
  );
}

/** The listeners' info page: its photo here; its words in Configuration
 * (info, kiwi.name and location, station). */
function InfoPage() {
  const [photo, setPhoto] = useState<boolean | null>(null);
  const [v, setV] = useState(0);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const load = () => get<{ photo: boolean }>("api/info").then((i) => setPhoto(i.photo));
  useEffect(() => void load(), []);
  const upload = async (e: Event) => {
    const input = e.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    input.value = "";
    if (!file) return;
    setMsg(null);
    try {
      await putText("api/admin/info/photo", file, file.type || "application/octet-stream");
      setMsg({ ok: true, text: `Saved ${file.name}.` });
      setV(v + 1);
      await load();
    } catch (err) {
      setMsg({ ok: false, text: `${file.name}: ${(err as Error).message}` });
    }
  };
  const remove = async () => {
    if (!confirm("Remove the photo?")) return;
    await call("DELETE", "api/admin/info/photo").catch((e: Error) => setMsg({ ok: false, text: e.message }));
    await load();
  };
  return (
    <div class="cards">
      <section class="card">
        <h3>Photo</h3>
        {photo && <img class="info-photo" src={`${href("api/info/photo")}?v=${v}`} alt="The receiver's photo" />}
        <p>
          <label class="file">
            {photo ? "Replace it" : "Upload one"} <input type="file" accept="image/jpeg,image/png,image/webp" onChange={(e) => void upload(e)} />
          </label>
          {photo && (
            <>
              {" · "}
              <button onClick={() => void remove()}>Remove</button>
            </>
          )}
        </p>
        {msg && <p class={msg.ok ? "ok" : "warn"}>{msg.text}</p>}
        <p class="dim">JPEG, PNG or WebP, at most 3 MB: the antenna, the station, the view. Kept beside the configuration file.</p>
      </section>
      <section class="card">
        <h3>What it says</h3>
        <p>
          Listeners open it with <b>Info</b> on the receiver page. It shows the receiver's name and location (Configuration: kiwi.name,
          kiwi.location), the station's callsign, locator and antenna (station), your words (info.description, info.contact), the photo,
          and the limits they meet (limits).
        </p>
        <p>
          <a href="#/admin/config">Configuration</a> · <a href="./#info">See it as listeners do</a>
        </p>
      </section>
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

const mb = (n: number) => `${(n / 1e6).toFixed(0)} MB`;
const day = (iso: string | null) => (iso ? iso.slice(0, 16).replace("T", " ") + " UTC" : "–");

/** Wait for the unit to come back after a reboot, then reload. */
async function backAfterReboot(): Promise<void> {
  const t0 = Date.now();
  // it goes down first
  await new Promise((r) => setTimeout(r, 15_000));
  while (Date.now() - t0 < 15 * 60_000) {
    try {
      await get("api/auth");
      location.reload();
      return;
    } catch {
      await new Promise((r) => setTimeout(r, 5000));
    }
  }
}

/** OS updates: the running version, the newest release, its install
 * into the other root slot and the reboot into it. */
function UpdatePage() {
  const [v, setV] = useState<UpdateView | null>(null);
  const [busy, setBusy] = useState("");
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const [rebooting, setRebooting] = useState(false);
  const running = v?.install.phase === "running";
  usePoll(
    async () => {
      if (!rebooting) setV(await get<UpdateView>("api/admin/update"));
    },
    running ? 1000 : 10_000,
  );
  const act = async (what: string, f: () => Promise<UpdateView | null>) => {
    setBusy(what);
    setMsg(null);
    try {
      const r = await f();
      if (r) setV(r);
    } catch (e) {
      setMsg({ ok: false, text: (e as Error).message });
    } finally {
      setBusy("");
    }
  };
  if (!v) return <p class="dim">Loading…</p>;
  if (!v.available) {
    return (
      <div class="cards">
        <section class="card">
          <h3>OS updates</h3>
          <p>Not available: {v.why}.</p>
        </section>
      </div>
    );
  }
  const i = v.install;
  const rel = v.latest;
  const check = () => act("check", () => call<UpdateView>("POST", "api/admin/update/check"));
  const install = (version: string) => {
    if (!confirm(`Install web888-ng ${version} into the other root slot? The receiver keeps running; a reboot then starts the new system.`)) return;
    void act("install", () => call<UpdateView>("POST", "api/admin/update/install", { version }));
  };
  const reboot = () => {
    if (!confirm("Reboot the unit now? Everyone is disconnected; it is back in a minute or two.")) return;
    void act("reboot", async () => {
      await call("POST", "api/admin/update/reboot");
      setRebooting(true);
      void backAfterReboot();
      return null;
    });
  };
  if (rebooting) {
    return (
      <div class="cards">
        <section class="card">
          <h3>Rebooting</h3>
          <p>The unit is restarting{i.phase === "done" ? ` into web888-ng ${i.version}` : ""}. This page reloads when it is back.</p>
          <p class="dim">
            If the new system doesn't come up, the unit goes back to the old one by itself after three tries (a few minutes).
          </p>
        </section>
      </div>
    );
  }
  return (
    <div class="cards">
      <section class="card">
        <h3>OS</h3>
        <dl>
          <dt>Running</dt>
          <dd>web888-ng {v.current ?? "?"}{v.booted ? `, slot ${v.booted}` : ""}</dd>
          <dt>Newest release</dt>
          <dd>
            {rel ? (
              <>
                <a href={rel.page} target="_blank" rel="noopener">
                  {rel.version}
                </a>{" "}
                <span class="dim">
                  ({day(rel.published)}, {mb(rel.size)}
                  {rel.prerelease ? ", pre-release" : ""})
                </span>
              </>
            ) : v.checked ? (
              "none found"
            ) : (
              "not looked yet"
            )}
          </dd>
          <dt>Looked</dt>
          <dd>
            {v.checked ? `${ago(v.checked)} ago` : "never"}
            {v.check_hours > 0 && v.enabled ? <span class="dim"> (by itself every {v.check_hours} h)</span> : null}
          </dd>
        </dl>
        {v.check_error && <p class="warn">Looking for releases failed: {v.check_error}</p>}
        {!v.enabled && <p class="warn">Updates are off (Configuration: update.enabled).</p>}
        <p>
          <button onClick={() => void check()} disabled={!!busy || !v.enabled}>
            {busy === "check" ? "Looking…" : "Look for a release"}
          </button>{" "}
          {rel && v.newer && i.phase !== "done" && (
            <button class="on" onClick={() => install(rel.version)} disabled={!!busy || running || !v.enabled}>
              Install {rel.version}
            </button>
          )}
          {rel && !v.newer && v.checked && <span class="ok">Up to date.</span>}
        </p>
        {rel && !v.newer && i.phase === "idle" && (
          <p class="dim">
            <a
              href="#"
              onClick={(e) => {
                e.preventDefault();
                install(rel.version);
              }}
            >
              Install {rel.version} again
            </a>{" "}
            (into the other slot, as an update would).
          </p>
        )}
        {rel?.notes && (
          <details>
            <summary>Release notes</summary>
            <pre class="notes">{rel.notes}</pre>
          </details>
        )}
      </section>
      {i.phase !== "idle" && (
        <section class="card">
          <h3>Install {i.version}</h3>
          {i.phase === "running" && (
            <>
              <progress max={100} value={i.percent} />
              <p>
                {i.percent} % · {i.step}
              </p>
              <p class="dim">Streamed from the release and checked as it goes; the receiver keeps running.</p>
            </>
          )}
          {i.phase === "done" && (
            <>
              <p class="ok">Installed{i.ended && i.started ? ` in ${Math.round((i.ended - i.started) / 60)} min` : ""}. Reboot to start it.</p>
              <p>
                <button class="on" onClick={reboot} disabled={!!busy}>
                  Reboot now
                </button>
              </p>
              <p class="dim">The new system boots first, with three tries; once web888d runs for a minute it is kept. Settings stay.</p>
            </>
          )}
          {i.phase === "failed" && <p class="warn">Failed: {i.error}. The running system is unchanged.</p>}
          {i.log.length > 0 && (
            <details open={i.phase === "failed"}>
              <summary>RAUC</summary>
              <pre class="notes">{i.log.join("\n")}</pre>
            </details>
          )}
        </section>
      )}
      <section class="card">
        <h3>Root slots</h3>
        {v.status_error && <p class="warn">{v.status_error}</p>}
        <table>
          <thead>
            <tr>
              <th>Slot</th>
              <th>State</th>
              <th>Boot</th>
              <th>Version</th>
              <th>Installed (UTC)</th>
            </tr>
          </thead>
          <tbody>
            {v.slots.map((s) => (
              <tr>
                <td>
                  {s.bootname ?? s.name} <span class="dim">{s.name}</span>
                </td>
                <td>{s.state ?? "–"}</td>
                <td class={s.boot_status === "bad" ? "warn" : ""}>{s.boot_status ?? "–"}</td>
                <td>{s.version ?? "–"}</td>
                <td>{s.installed ? s.installed.slice(0, 16).replace("T", " ") : "–"}</td>
              </tr>
            ))}
          </tbody>
        </table>
        <p class="dim">
          An update goes into the slot not running; the old one stays as it was, to go back to. The boot loader on the card (BOOT.BIN,
          u-boot.img) is not part of it: see Boot loader below.
        </p>
      </section>
      {msg && <p class={msg.ok ? "ok" : "warn"}>{msg.text}</p>}
    </div>
  );
}

/** Settings backup (a JSON file to download) and restore (one uploaded). */
function BackupCard() {
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const restore = async (f: File) => {
    setMsg(null);
    let text: string;
    try {
      text = await f.text();
      JSON.parse(text);
    } catch {
      setMsg({ ok: false, text: `${f.name}: not a settings backup (JSON)` });
      return;
    }
    if (!confirm(`Restore the settings from ${f.name}? The configuration, the passwords and the own labels are replaced by the file's.`)) return;
    setBusy(true);
    try {
      const r = await call<Changes & { labels: number | null; from: { server: string; created: number } }>(
        "POST",
        "api/admin/restore",
        JSON.parse(text),
      );
      const when = new Date(r.from.created * 1000).toISOString().slice(0, 16).replace("T", " ");
      const parts = [`Restored the settings saved by web888d ${r.from.server} on ${when} UTC`];
      if (r.labels !== null) parts.push(`${r.labels} own labels`);
      if (r.restart.length) parts.push(`restart web888d for: ${r.restart.join(", ")}`);
      setMsg({ ok: true, text: parts.join("; ") + "." });
    } catch (e) {
      setMsg({ ok: false, text: `Not restored: ${(e as Error).message}` });
    } finally {
      setBusy(false);
    }
  };
  return (
    <div class="cards">
      <section class="card">
        <h3>Backup</h3>
        <p>
          <a href={href("api/admin/backup")} download>
            Download the settings
          </a>
          {" · "}
          <label class="file">
            {busy ? "Restoring…" : "Restore from a file"}{" "}
            <input
              type="file"
              accept=".json,application/json"
              disabled={busy}
              onChange={(e) => {
                const f = (e.currentTarget as HTMLInputElement).files?.[0];
                (e.currentTarget as HTMLInputElement).value = "";
                if (f) void restore(f);
              }}
            />
          </label>
        </p>
        {msg && <p class={msg.ok ? "ok" : "warn"}>{msg.text}</p>}
        <p class="dim">
          One file with this page's configuration, the passwords (as hashes: keep the file private) and the own labels. The card's
          own settings (network, tunnels, SSH key) are in web888.txt on the card, not here.
        </p>
      </section>
    </div>
  );
}

interface BootLoader {
  available: boolean;
  state?: "current" | "update" | "unsupported";
  reason?: string;
  blocked?: string;
  slot?: string;
  shipped_version?: string;
  installed_spl?: string;
  installed_uboot?: string;
  shipped_spl?: string;
  shipped_uboot?: string;
  old_kept?: string;
}

/** The card's boot loader: written from the running slot's copy when the
 * owner asks (os docs/updates.md). */
function BootLoaderCard() {
  const [b, setB] = useState<BootLoader | null>(null);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  usePoll(async () => {
    if (!busy) setB(await get<BootLoader>("api/admin/bootloader"));
  }, 30_000);
  if (!b) return null;
  const short = (h?: string) => (h ? h.slice(0, 12) : "?");
  const install = async () => {
    if (
      !confirm(
        "Write the boot loader to the card now?\n\n" +
          "Unlike the system, the boot loader has no second copy to fall back to. If the unit loses power while it is written, " +
          "or the new boot loader doesn't start, the unit won't boot until the card is fixed on a computer (rename boot.bin.old " +
          "and u-boot.img.old back) or re-imaged. Download a settings backup first, and have a card reader at hand.\n\n" +
          "The receiver keeps running; the new boot loader starts at the next reboot.",
      )
    )
      return;
    setBusy(true);
    setMsg(null);
    try {
      const r = await call<{ output: string }>("POST", "api/admin/bootloader/install");
      setMsg({ ok: true, text: r.output.split("\n").pop() ?? "done" });
      setB(await get<BootLoader>("api/admin/bootloader"));
    } catch (e) {
      setMsg({ ok: false, text: (e as Error).message });
    } finally {
      setBusy(false);
    }
  };
  return (
    <div class="cards">
      <section class="card">
        <h3>Boot loader</h3>
        {!b.available || b.state === "unsupported" ? (
          <p class="dim">Not updated from here: {b.reason}.</p>
        ) : (
          <>
            <dl>
              <dt>On the card</dt>
              <dd>
                {b.state === "current" ? (
                  <span class="ok">this system's{b.shipped_version ? ` (web888-ng ${b.shipped_version})` : ""}</span>
                ) : (
                  <span class="warn">another one</span>
                )}{" "}
                <span class="dim">
                  SPL {short(b.installed_spl)}, U-Boot {short(b.installed_uboot)}
                </span>
              </dd>
              {b.state === "update" && (
                <>
                  <dt>This system's</dt>
                  <dd>
                    web888-ng {b.shipped_version ?? "?"}{" "}
                    <span class="dim">
                      SPL {short(b.shipped_spl)}, U-Boot {short(b.shipped_uboot)}
                    </span>
                  </dd>
                </>
              )}
              {b.old_kept && (
                <>
                  <dt>Kept</dt>
                  <dd class="dim">boot.bin.old, u-boot.img.old (the one before)</dd>
                </>
              )}
            </dl>
            {b.state === "update" && (
              <>
                {b.blocked && <p class="warn">Not now: {b.blocked}.</p>}
                <p>
                  <button class="on" onClick={() => void install()} disabled={busy || !!b.blocked}>
                    {busy ? "Writing…" : "Write it to the card…"}
                  </button>
                </p>
                <p class="dim">
                  Not needed for the system to run: a newer boot loader comes with some releases (their notes say why). Be prepared to
                  re-image the card if something unexpected happens.
                </p>
              </>
            )}
          </>
        )}
        {msg && <p class={msg.ok ? "ok" : "warn"}>{msg.text}</p>}
      </section>
    </div>
  );
}

type LogSource = "web888d" | "boot" | "kernel" | "setup";
const LOG_SOURCES: { id: LogSource; name: string }[] = [
  { id: "web888d", name: "web888d" },
  { id: "boot", name: "System (this boot)" },
  { id: "kernel", name: "Kernel" },
  { id: "setup", name: "web888-setup.log" },
];

interface Logs {
  source: LogSource;
  lines: string[];
  error?: string;
  sources: Record<LogSource, boolean>;
}

/** The system's logs: the last lines of one source, refreshed. */
function LogsPage() {
  const [src, setSrc] = useState<LogSource>("web888d");
  const [n, setN] = useState(300);
  const [filter, setFilter] = useState("");
  const [follow, setFollow] = useState(true);
  const [logs, setLogs] = useState<Logs | null>(null);
  const [err, setErr] = useState("");
  const pre = useRef<HTMLPreElement>(null);
  usePoll(
    async () => {
      if (!follow && logs?.source === src) return;
      try {
        setLogs(await get<Logs>(`api/admin/logs?source=${src}&lines=${n}`));
        setErr("");
      } catch (e) {
        setErr((e as Error).message);
      }
    },
    5000,
  );
  // a new source or length: at once
  useEffect(() => {
    get<Logs>(`api/admin/logs?source=${src}&lines=${n}`)
      .then((l) => {
        setLogs(l);
        setErr("");
      })
      .catch((e: Error) => setErr(e.message));
  }, [src, n]);
  const f = filter.trim().toLowerCase();
  const lines = (logs?.source === src ? logs.lines : []).filter((l) => !f || l.toLowerCase().includes(f));
  useEffect(() => {
    if (follow && pre.current) pre.current.scrollTop = pre.current.scrollHeight;
  }, [logs, follow, filter]);
  const save = () => {
    const blob = new Blob([lines.join("\n") + "\n"], { type: "text/plain" });
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = `web888-${src}-${new Date().toISOString().slice(0, 19).replace(/[:T]/g, "-")}.log`;
    a.click();
    URL.revokeObjectURL(a.href);
  };
  return (
    <div class="logs">
      <div class="logbar">
        <nav class="seg">
          {LOG_SOURCES.map((s) => (
            <button
              key={s.id}
              class={src === s.id ? "on" : ""}
              disabled={logs ? !logs.sources[s.id] : false}
              title={logs && !logs.sources[s.id] ? "not on this system" : ""}
              onClick={() => setSrc(s.id)}
            >
              {s.name}
            </button>
          ))}
        </nav>
        <select value={n} onChange={(e) => setN(Number((e.currentTarget as HTMLSelectElement).value))}>
          {[100, 300, 1000, 5000].map((k) => (
            <option value={k}>{k} lines</option>
          ))}
        </select>
        <input type="search" placeholder="Filter" value={filter} onInput={(e) => setFilter((e.currentTarget as HTMLInputElement).value)} />
        <label>
          <input type="checkbox" checked={follow} onChange={(e) => setFollow((e.currentTarget as HTMLInputElement).checked)} /> Follow
        </label>
        <button onClick={save} disabled={!lines.length}>
          Download
        </button>
      </div>
      {err && <p class="warn">{err}</p>}
      {logs?.error && logs.source === src && <p class="warn">{logs.error}</p>}
      {logs && logs.source === src && !logs.error && !lines.length && (
        <p class="dim">{logs.lines.length ? "No line matches the filter." : "No lines yet."}</p>
      )}
      <pre class="log" ref={pre}>
        {lines.join("\n")}
      </pre>
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
  // the tab bar scrolls sideways on a phone: keep the open tab in sight
  useEffect(() => {
    document.querySelector(".admin header nav .on")?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [tab.value, auth]);
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
        {tab.value === "config" && (
          <>
            <ConfigPage onRestart={() => void restart()} />
            <BackupCard />
          </>
        )}
        {tab.value === "labels" && <LabelsPage />}
        {tab.value === "info" && <InfoPage />}
        {tab.value === "passwords" && <PasswordsPage />}
        {tab.value === "update" && (
          <>
            <UpdatePage />
            <BootLoaderCard />
          </>
        )}
        {tab.value === "logs" && <LogsPage />}
      </div>
    </div>
  );
}
