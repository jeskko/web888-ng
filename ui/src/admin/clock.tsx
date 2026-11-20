// SPDX-License-Identifier: GPL-3.0-or-later
// The admin page's Clock tab (GET api/admin/clock, server/doc/api.md
// §8.5): the system clock as chrony keeps it, chrony's sources (the
// receiver's own LF among its NTP servers), what the receiver sends
// chrony, and the time-code stations it hears.

import { useState } from "preact/hooks";
import { t, tn } from "../i18n.ts";
import { get } from "./api.ts";
import { ago, usePoll } from "./util.ts";

interface Tracking {
  ref_id?: string;
  ref_name?: string;
  stratum?: number;
  slow_s?: number;
  last_offset_s?: number;
  rms_offset_s?: number;
  freq_ppm?: number;
  residual_freq_ppm?: number;
  skew_ppm?: number;
  root_delay_s?: number;
  root_dispersion_s?: number;
  update_interval_s?: number;
  leap?: string;
}
interface ChronySource {
  mode?: string;
  state?: string;
  name?: string;
  stratum?: number;
  poll?: number;
  reach?: number;
  last_rx_s?: number;
  offset_s?: number;
  error_s?: number;
}
interface TcStation {
  t: number;
  station: string;
  bin_khz: number;
  carrier_dbfs: number | null;
  snr_db_hz: number | null;
  ok: boolean;
  confirmed: boolean;
  late_ms: number | null;
  adc_ppm: number | null;
  floor_dbfs_hz: number | null;
  raw_ppm: number | null;
  sys_ppm: number | null;
  temp_c: number | null;
  offset_ms: number | null;
  sigma_ms: number | null;
  shed: boolean;
  cpu_pct: number | null;
  minutes: { ok: number; unconfirmed: number; failed: number };
}
interface SyncSource {
  offset: number | null;
  sigma: number | null;
  used: number;
  rejected: number;
}
interface Clock {
  now: number;
  chrony: { available: boolean; tracking?: Tracking | null; sources?: ChronySource[] };
  kernel: { synced: boolean; esterror_s: number; maxerror_s: number; freq_ppm: number } | null;
  timebase: { source: string; uncertainty_s: number; adc_ppm: number | null } | null;
  timesync: {
    sources: Record<string, SyncSource>;
    offset: number | null;
    sigma: number | null;
    n: number;
    sent: number;
    send_errors: number;
    rejected: number;
  } | null;
  timecode: { stations: TcStation[] } | null;
}

/** Seconds as µs or ms, signed when `sign`. */
function secs(s: number | null | undefined, sign = false): string {
  if (s === null || s === undefined || !Number.isFinite(s)) return "–";
  const a = Math.abs(s);
  const p = sign && s > 0 ? "+" : "";
  if (a < 1e-5) return `${p}${(s * 1e6).toFixed(1)} µs`;
  if (a < 1e-3) return `${p}${(s * 1e6).toFixed(0)} µs`;
  if (a < 1) return `${p}${(s * 1e3).toFixed(a < 0.01 ? 2 : 1)} ms`;
  return `${p}${s.toFixed(3)} s`;
}

function stateText(s: string | undefined): string {
  switch (s) {
    // i18n: chrony's states of a time source; selected = the one the clock follows; combined = averaged in with it; falseticker = disagrees with the others
    case "*":
      return t("selected");
    case "+":
      return t("combined");
    case "-":
      return t("not combined");
    case "?":
      return t("unusable");
    case "x":
      return t("a falseticker");
    case "~":
      return t("too variable");
    default:
      return s ?? "–";
  }
}

/** The last 8 polls, oldest first: ■ answered, □ not. */
function reach(r: number | undefined): string {
  if (r === undefined) return "–";
  let s = "";
  for (let k = 7; k >= 0; k--) s += (r >> k) & 1 ? "■" : "□";
  return s;
}

function SystemCard({ c }: { c: Clock }) {
  const tr = c.chrony.tracking;
  const k = c.kernel;
  return (
    <section class="card">
      <h3>{t("System clock")}</h3>
      {!c.chrony.available && <p class="warn">{t("chrony doesn't answer (chronyc): the kernel's view only.")}</p>}
      <dl>
        <dt>{t("Synchronised")}</dt>
        <dd class={k && !k.synced ? "warn" : ""}>{k ? (k.synced ? t("yes") : t("no")) : "–"}</dd>
        {tr && (
          <>
            {/* i18n: the reference = the time source the clock follows now; LF = this receiver's own time-code stations */}
            <dt>{t("Reference")}</dt>
            <dd>
              {tr.ref_name === "LF" || tr.ref_id === "4C460000" ? t("LF: this receiver's time-code stations") : tr.ref_name}
              {tr.stratum !== undefined && <span class="dim"> {t("stratum {n}", { n: tr.stratum })}</span>}
            </dd>
            {/* i18n: how far the system clock is from the reference now, and which way */}
            <dt>{t("Offset")}</dt>
            <dd>
              {tr.slow_s === undefined
                ? "–"
                : tr.slow_s >= 0
                  ? t("{s} slow", { s: secs(tr.slow_s) })
                  : t("{s} fast", { s: secs(-tr.slow_s) })}
              <span class="dim"> {t("(RMS {s})", { s: secs(tr.rms_offset_s) })}</span>
            </dd>
            {/* i18n: the crystal's rate error that chrony corrects; skew = its uncertainty */}
            <dt>{t("Frequency")}</dt>
            <dd>
              {tr.freq_ppm === undefined
                ? "–"
                : tr.freq_ppm < 0
                  ? t("{ppm} ppm slow", { ppm: (-tr.freq_ppm).toFixed(3) })
                  : t("{ppm} ppm fast", { ppm: tr.freq_ppm.toFixed(3) })}
              <span class="dim"> ±{tr.skew_ppm?.toFixed(3)} ppm</span>
            </dd>
            {/* i18n: the largest error the clock may have against true time: half the path's delay plus the dispersion */}
            <dt>{t("Error bound")}</dt>
            <dd>{secs((tr.root_delay_s ?? 0) / 2 + (tr.root_dispersion_s ?? 0))}</dd>
            <dt>{t("Updated every")}</dt>
            <dd>{tr.update_interval_s !== undefined ? `${tr.update_interval_s.toFixed(0)} s` : "–"}</dd>
            {/* i18n: leap second status: Normal, Insert second, Delete second, Not synchronised */}
            <dt>{t("Leap status")}</dt>
            <dd>{tr.leap ?? "–"}</dd>
          </>
        )}
        {k && !tr && (
          <>
            <dt>{t("Estimated error")}</dt>
            <dd>{secs(k.esterror_s)}</dd>
            <dt>{t("Frequency")}</dt>
            <dd>{t("{ppm} ppm correction", { ppm: k.freq_ppm.toFixed(3) })}</dd>
          </>
        )}
      </dl>
    </section>
  );
}

function SourcesCard({ c }: { c: Clock }) {
  const src = c.chrony.sources ?? [];
  return (
    <section class="card span2">
      {/* i18n: chrony = the program that keeps the system clock; its sources = NTP servers and reference clocks */}
      <h3>{t("chrony's sources")}</h3>
      {src.length === 0 ? (
        <p class="dim">{t("None.")}</p>
      ) : (
        <table>
          <thead>
            <tr>
              <th>{t("source")}</th>
              <th>{t("state")}</th>
              {/* i18n: reach = which of the last 8 polls got an answer */}
              <th title={t("The last 8 polls, oldest first: ■ answered, □ not")}>{t("reach")}</th>
              <th class="r">{t("last")}</th>
              {/* i18n: the system clock against the source at its last sample (+: the clock is ahead), ± its error bound */}
              <th class="r" title={t("The system clock against the source at its last sample (+: the clock is ahead), ± its error bound")}>
                {t("offset")}
              </th>
            </tr>
          </thead>
          <tbody>
            {src.map((s, k) => (
              <tr key={k} class={s.state === "*" || s.state === "+" ? "" : "dim"}>
                <td>
                  {s.mode === "#" ? (s.name === "LF" ? t("LF (time-code stations)") : s.name) : s.name}
                  {s.stratum !== undefined && s.mode !== "#" && <span class="dim"> ({s.stratum})</span>}
                </td>
                <td class={s.state === "x" || s.state === "~" ? "warn" : ""}>{stateText(s.state)}</td>
                <td style="font-family: monospace">{reach(s.reach)}</td>
                <td class="r">{s.last_rx_s !== undefined && s.last_rx_s < 4e9 ? `${s.last_rx_s.toFixed(0)} s` : "–"}</td>
                <td class="r">
                  {secs(s.offset_s, true)} <span class="dim">±{secs(s.error_s)}</span>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}

function SyncCard({ c }: { c: Clock }) {
  const s = c.timesync;
  if (!s) return null;
  const names = Object.keys(s.sources);
  return (
    <section class="card">
      {/* i18n: what this receiver sends to chrony each minute: its time sources combined */}
      <h3>{t("To chrony")}</h3>
      <dl>
        {/* i18n: the combined sample: the system clock's offset (true time minus the clock's) ± its standard error */}
        <dt>{t("Last sample")}</dt>
        <dd>
          {s.offset === null ? "–" : `${secs(s.offset, true)} ± ${secs(s.sigma)}`}{" "}
          {s.offset !== null && <span class="dim">{tn("from {n} source", "from {n} sources", s.n)}</span>}
        </dd>
        <dt>{t("Samples")}</dt>
        <dd class={s.send_errors > 0 && s.sent === 0 ? "warn" : ""}>
          {t("{sent} sent, {failed} failed (no chrony), {none} minutes with none usable", {
            sent: s.sent,
            failed: s.send_errors,
            none: s.rejected,
          })}
        </dd>
      </dl>
      {names.length > 0 && (
        <table>
          <thead>
            <tr>
              <th>{t("source")}</th>
              <th class="r">{t("offset")}</th>
              {/* i18n: minutes the source was in the combined sample / left out of it */}
              <th class="r">{t("used")}</th>
              <th class="r">{t("left out")}</th>
            </tr>
          </thead>
          <tbody>
            {names.map((n) => {
              const v = s.sources[n];
              return (
                <tr key={n}>
                  <td>{n}</td>
                  <td class="r">
                    {secs(v.offset, true)} <span class="dim">±{secs(v.sigma)}</span>
                  </td>
                  <td class="r">{v.used}</td>
                  <td class="r">{v.rejected}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </section>
  );
}

function StationsCard({ c }: { c: Clock }) {
  const st = c.timecode?.stations ?? [];
  if (!c.timecode) {
    return (
      <section class="card">
        <h3>{t("Time-code stations")}</h3>
        <p class="dim">{t("Not watched: needs [timecode] enabled in the config and the channelizer's bins.")}</p>
      </section>
    );
  }
  const first = st[0];
  const bins = [...new Map(st.map((s) => [s.bin_khz, s])).values()];
  const adc = st.filter((s) => s.adc_ppm !== null).map((s) => s.adc_ppm!);
  adc.sort((a, b) => a - b);
  return (
    <section class="card">
      {/* i18n: LF time-code stations (DCF77, MSF, RBU, DCF39 ...) decoded on the receiver */}
      <h3>{t("Time-code stations")}</h3>
      {first && <p class="dim">{t("Minute {when} UTC ({ago} ago).", { when: new Date(first.t * 1000).toISOString().slice(11, 16), ago: ago(first.t + 60) })}</p>}
      <table>
        <thead>
          <tr>
            <th>{t("station")}</th>
            {/* i18n: SNR = signal-to-noise ratio of the carrier in 1 Hz */}
            <th class="r">{t("SNR")}</th>
            {/* i18n: minutes confirmed / decoded but not confirmed / not decoded since the start */}
            <th class="r" title={t("Minutes confirmed / decoded but unconfirmed / not decoded")}>
              {t("minutes")}
            </th>
            {/* i18n: late = how late the station's time arrives by the system clock (ms); for EFR, after its 10 s mark */}
            <th class="r">{t("late")}</th>
            {/* i18n: the system clock's error by this station (true minus system time), ± its scatter */}
            <th class="r" title={t("The system clock's error by this station (true minus system time, ms), ± the scatter of its last minutes")}>
              {t("clock offset")}
            </th>
          </tr>
        </thead>
        <tbody>
          {st.map((s) => (
            <tr key={s.station} class={s.shed ? "dim" : ""}>
              <td>
                {s.station} <span class="dim">{s.bin_khz.toFixed(0)}</span>
                {s.shed && <span class="warn"> {t("(shed)")}</span>}
              </td>
              <td class="r">{s.snr_db_hz === null ? "–" : `${s.snr_db_hz.toFixed(0)} dB`}</td>
              <td class="r">
                {s.minutes.ok}/{s.minutes.unconfirmed}/{s.minutes.failed}
              </td>
              <td class={`r ${s.confirmed ? "" : "dim"}`}>{s.late_ms === null ? "–" : `${s.late_ms.toFixed(2)} ms`}</td>
              <td class="r">
                {s.offset_ms === null ? "–" : `${s.offset_ms > 0 ? "+" : ""}${s.offset_ms.toFixed(2)} ms`}{" "}
                {s.sigma_ms !== null && <span class="dim">±{s.sigma_ms.toFixed(2)}</span>}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <dl>
        {/* i18n: the noise floor of each frequency bin the stations are watched in */}
        <dt>{t("Noise floor")}</dt>
        <dd>{bins.map((b) => `${b.bin_khz.toFixed(0)} kHz: ${b.floor_dbfs_hz?.toFixed(1) ?? "–"} dBFS/Hz`).join(", ")}</dd>
        {/* i18n: the ADC's sample clock error, measured on the stations' carriers (atomic clocks) */}
        <dt>{t("ADC clock")}</dt>
        <dd>{adc.length ? t("{ppm} ppm (carriers)", { ppm: (adc[Math.floor(adc.length / 2)] > 0 ? "+" : "") + adc[Math.floor(adc.length / 2)].toFixed(3) }) : "–"}</dd>
        {/* i18n: the CPU's crystal as it runs (without NTP's correction) against the ADC clock */}
        <dt>{t("CPU crystal")}</dt>
        <dd>
          {first?.raw_ppm == null ? "–" : t("{ppm} ppm against the ADC clock", { ppm: first.raw_ppm.toFixed(3) })}
          {first?.temp_c != null && <span class="dim"> {first.temp_c.toFixed(1)} °C</span>}
        </dd>
        <dt>{t("Monitor CPU")}</dt>
        <dd>{first?.cpu_pct == null ? "–" : t("{pct} % of a core", { pct: first.cpu_pct.toFixed(1) })}</dd>
      </dl>
    </section>
  );
}

function TimebaseCard({ c }: { c: Clock }) {
  const tb = c.timebase;
  if (!tb) return null;
  return (
    <section class="card">
      {/* i18n: the receiver's timebase: how sample times map to UTC */}
      <h3>{t("Receiver timebase")}</h3>
      <dl>
        <dt>{t("Source")}</dt>
        <dd>{tb.source}</dd>
        <dt>{t("Uncertainty")}</dt>
        <dd>{secs(tb.uncertainty_s)}</dd>
        {/* i18n: the sample stream's rate against the system clock */}
        <dt>{t("Stream rate")}</dt>
        <dd>{tb.adc_ppm === null ? "–" : `${tb.adc_ppm.toFixed(3)} ppm`}</dd>
      </dl>
    </section>
  );
}

export function ClockPage() {
  const [c, setC] = useState<Clock | null>(null);
  usePoll(async () => setC(await get<Clock>("api/admin/clock")), 5000);
  if (!c) return <p class="dim">{t("Loading…")}</p>;
  return (
    <div class="cards">
      <SystemCard c={c} />
      <SourcesCard c={c} />
      <SyncCard c={c} />
      <StationsCard c={c} />
      <TimebaseCard c={c} />
    </div>
  );
}

