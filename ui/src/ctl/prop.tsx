// SPDX-License-Identifier: GPL-3.0-or-later
// The HF propagation panel (`/api/propagation`): for the hour now, how
// well each band carries from each region to this receiver; for one
// region, the whole day. The Prop button opens it, as does a link to the
// page with #propagation. A band's name tunes there.

import { signal } from "@preact/signals";
import { useEffect, useState } from "preact/hooks";
import { bandHz, bandName, cellTitle, forecastBasis, forecastTitle, relColor, stormNote, type PropReply } from "../prop.ts";
import { t } from "../i18n.ts";
import { api, radio, tuneInView } from "../state.ts";
import { cssGradient } from "../view/colormap.ts";

export const propOpen = signal(location.hash === "#propagation");
/** The mode and the region shown (-1: all regions, the hour now). */
const modeIx = signal(0);
const regionIx = signal(-1);

const utcHour = () => new Date().getUTCHours();

/** A band's name ("20 m"; short: "20"), a button that tunes there when
 *  the radio reaches it. */
function BandLabel({ mhz, short = false }: { mhz: number; short?: boolean }) {
  const r = radio.value;
  const hz = bandHz(mhz);
  const can = r && hz >= r.rf_lo && hz <= r.rf_hi;
  const name = bandName(mhz);
  const text = short ? name.replace(/ m$/, "") : name;
  return can ? (
    <button class="prop-band" title={t("{band}: tune to {khz} kHz", { band: name, khz: (hz / 1e3).toFixed(0) })} onClick={() => tuneInView(hz)}>
      {text}
    </button>
  ) : (
    <span title={name}>{text}</span>
  );
}

/** Every region, every band, at the hour now. */
function NowTable({ d, hour }: { d: PropReply & { forecast: NonNullable<PropReply["forecast"]> }; hour: number }) {
  const f = d.forecast;
  const m = modeIx.value;
  return (
    <table class="prop-grid prop-now">
      <thead>
        <tr>
          <th class="dim">m</th>
          {f.bands.map((b) => (
            <th key={b}>
              <BandLabel mhz={b} short />
            </th>
          ))}
          {/* i18n: MUF = maximum usable frequency */}
          <th title={t("Maximum usable frequency now (MHz)")}>MUF</th>
        </tr>
      </thead>
      <tbody>
        {f.paths.map((p, i) => (
          <tr key={p.name}>
            <th class="prop-region">
              <button class="prop-link" onClick={() => (regionIx.value = i)} title={t("The whole day for this region")}>
                {p.name}
              </button>
              <span class="dim prop-dist">
                {Math.round(p.km / 100) * 100} km, {Math.round(p.bearing)}°
              </span>
            </th>
            {p.error ? (
              <td colSpan={f.bands.length + 1} class="dim">
                {p.error}
              </td>
            ) : (
              <>
                {f.bands.map((_, b) => {
                  const r = p.rel[m]?.[b]?.[hour] ?? null;
                  return (
                    <td key={b} class="prop-cell" style={{ background: relColor(r) }} title={cellTitle(f, p, m, b, hour)}>
                      <span>{r === null ? "" : Math.round(r * 100)}</span>
                    </td>
                  );
                })}
                <td class="prop-muf">{p.muf[hour]?.toFixed(1) ?? ""}</td>
              </>
            )}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/** One region through the day: bands (highest first) by UTC hour. */
function DayTable({ d, hour }: { d: PropReply & { forecast: NonNullable<PropReply["forecast"]> }; hour: number }) {
  const f = d.forecast;
  const p = f.paths[regionIx.value];
  const m = modeIx.value;
  if (!p) return null;
  if (p.error) return <p class="dim">{p.error}</p>;
  const hours = [...Array(24).keys()];
  const order = f.bands.map((_, i) => i).reverse();
  return (
    <table class="prop-grid prop-day">
      <thead>
        <tr>
          <th class="dim">UTC</th>
          {hours.map((h) => (
            <th key={h} class={h === hour ? "prop-nowcol" : ""}>
              {h % 3 === 0 ? String(h).padStart(2, "0") : ""}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        <tr>
          <th title={t("Maximum usable frequency (MHz)")}>MUF</th>
          {hours.map((h) => (
            <td key={h} class={`prop-muf${h === hour ? " prop-nowcol" : ""}`} title={p.muf[h] != null ? t("{hour} UTC: MUF {mhz} MHz", { hour: String(h).padStart(2, "0"), mhz: p.muf[h]! }) : ""}>
              {h % 3 === 0 && p.muf[h] != null ? Math.round(p.muf[h]) : ""}
            </td>
          ))}
        </tr>
        {order.map((b) => (
          <tr key={b}>
            <th>
              <BandLabel mhz={f.bands[b]} />
            </th>
            {hours.map((h) => {
              const r = p.rel[m]?.[b]?.[h] ?? null;
              return <td key={h} class={`prop-cell prop-hour${h === hour ? " prop-nowcol" : ""}`} style={{ background: relColor(r) }} title={cellTitle(f, p, m, b, h)} />;
            })}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function Body({ d }: { d: PropReply }) {
  const [hour, setHour] = useState(utcHour());
  useEffect(() => {
    const iv = setInterval(() => setHour(utcHour()), 30_000);
    return () => clearInterval(iv);
  }, []);
  if (!d.enabled) return <p class="dim">{t("The owner has turned the forecast off.")}</p>;
  const f = d.forecast;
  if (!f) return <p class="dim">{d.problem ?? t("The forecast is being made…")}</p>;
  const fd = { ...d, forecast: f };
  const storm = stormNote(d.kp);
  const region = f.paths[regionIx.value];
  return (
    <>
      <p class="prop-what">
        {forecastTitle(f)}
        {d.kp && (
          <span class="dim" title={t("Planetary K index at {time} UTC (NOAA); the highest in 24 hours {max}", { time: d.kp.time, max: d.kp.max24.toFixed(1) })}>
            {" "}
            · Kp {d.kp.kp.toFixed(1)}
          </span>
        )}
      </p>
      {storm && <p class="warn">{storm}</p>}
      <div class="prop-opts">
        <span class="seg">
          {f.modes.map((m, i) => (
            <button key={m.name} class={modeIx.value === i ? "on" : ""} onClick={() => (modeIx.value = i)} title={t("Signals that need {snr} dB SNR in 1 Hz", { snr: m.snr_db })}>
              {m.name}
            </button>
          ))}
        </span>
        <select value={regionIx.value} onChange={(e) => (regionIx.value = Number((e.target as HTMLSelectElement).value))} aria-label={t("Region")}>
          <option value={-1}>{t("All regions, now ({hour} UTC)", { hour: String(hour).padStart(2, "0") })}</option>
          {f.paths.map((p, i) => (
            <option key={p.name} value={i}>
              {t("{region}, the day", { region: p.name })}
            </option>
          ))}
        </select>
      </div>
      {region ? <DayTable d={fd} hour={hour} /> : <NowTable d={fd} hour={hour} />}
      <p class="prop-legend">
        <span class="dim">{t("Share of days a signal gets through:")}</span> <span class="prop-scale" style={{ background: cssGradient("viridis") }} /> <span class="dim">0 – 100 %</span>
      </p>
      <p class="dim prop-basis">
        {t("From each region to this receiver ({locator}).", { locator: f.rx.locator })} {forecastBasis(f)}
      </p>
      {d.fetch_errors.length > 0 && <p class="dim">{d.fetch_errors.join("; ")}</p>}
    </>
  );
}

/** The forecast as a dialog over the receiver. */
export function PropPanel() {
  const open = propOpen.value;
  const [d, setD] = useState<PropReply | null>(null);
  const [err, setErr] = useState("");
  useEffect(() => {
    if (!open) return;
    const load = () =>
      fetch(api("api/propagation"))
        .then((r) => (r.ok ? (r.json() as Promise<PropReply>) : Promise.reject(new Error(`${r.status}`))))
        .then((v) => {
          setD(v);
          setErr("");
        })
        .catch((e: Error) => setErr(t("The forecast didn't load ({msg}).", { msg: e.message })));
    void load();
    const iv = setInterval(load, 600_000);
    const key = (e: KeyboardEvent) => e.key === "Escape" && (propOpen.value = false);
    window.addEventListener("keydown", key);
    return () => {
      clearInterval(iv);
      window.removeEventListener("keydown", key);
    };
  }, [open]);
  if (!open) return null;
  return (
    <div class="overlay keyhelp" role="dialog" aria-label={t("HF propagation")} onClick={(e) => e.target === e.currentTarget && (propOpen.value = false)}>
      <div class="keyhelp-box info-box prop-box">
        <button class="info-x" onClick={() => (propOpen.value = false)} title={t("Close (Escape)")}>
          ✕
        </button>
        <h3>{t("HF propagation")}</h3>
        {d ? <Body d={d} /> : <p class="dim">{err || t("Loading…")}</p>}
      </div>
    </div>
  );
}

/** The header's Prop button. */
export function PropCtl() {
  return (
    <div class="group">
      <button class={propOpen.value ? "on" : ""} onClick={() => (propOpen.value = !propOpen.value)} title={t("HF propagation: which bands are open to where, this hour and through the day")}>
        {t("Prop")}
      </button>
    </div>
  );
}
