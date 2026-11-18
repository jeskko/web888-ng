// SPDX-License-Identifier: GPL-3.0-or-later
// HFDL decoded in this browser from this receiver's audio (USB on an HFDL
// channel): beside the waterfall, each decoded burst, newest first, from
// a ground station (squitters, logon confirms, uplinks) or an aircraft
// (logons, position reports, ACARS), with the aircraft named as far as
// it is known.

import { N_, t } from "../i18n.ts";
import { gsName, hfdlChannels, who, type HfdlItem, type HfdlMsg } from "../hfdl.ts";
import { clearHfdl, hfdlCount, hfdlError, hfdlMsgs, hfdlOn, hfdlStations, pickHfdl, setHfdl, st } from "../state.ts";

// i18n: kinds of HFDL message parts (the worker names them; ACARS stays as is): squitter = ground station announcement burst; logon/logoff = aircraft registering with / leaving a ground station; frequencies = list of channels the aircraft is told to use; system table = the ground stations' broadcast list of stations and channels; bad = part whose checksum failed
const KINDS: Record<string, string> = {
  squitter: N_("squitter"),
  position: N_("position"),
  frequencies: N_("frequencies"),
  logon: N_("logon"),
  "logon confirm": N_("logon confirm"),
  logoff: N_("logoff"),
  "logon denied": N_("logon denied"),
  "system table": N_("system table"),
  data: N_("data"),
  bad: N_("bad"),
  other: N_("other"),
};

const hhmmss = (utc: number) => new Date(utc * 1000).toISOString().slice(11, 19);

function where(m: HfdlMsg, gs: string): string {
  // i18n: squitter = the periodic broadcast burst a HFDL ground station (gs) sends to announce itself
  if (m.dir === "squitter") return t("{gs} squitter", { gs });
  return m.dir === "up" ? t("{gs} → aircraft", { gs }) : t("aircraft → {gs}", { gs });
}

function pos(it: HfdlItem): string {
  if (it.lat === null || it.lon === null) return "";
  const f = (v: number, p: string, n: string) => `${Math.abs(v).toFixed(2)}° ${v < 0 ? n : p}`;
  return `${f(it.lat, "N", "S")} ${f(it.lon, "E", "W")}`;
}

export function HfdlPanel() {
  if (!hfdlOn.value) return null;
  const msgs = hfdlMsgs.value;
  const stations = hfdlStations.value;
  const c = hfdlCount.value;
  const s = st.value;
  return (
    <aside class="spots hfdl">
      <div class="spots-bar">
        <strong>HFDL</strong>
        <select value="" onChange={(e) => pickHfdl(Number((e.target as HTMLSelectElement).value))} title={t("HFDL channels (USB, the dial on the listed frequency) and the ground stations using them")}>
          <option value="" disabled>
            {t("Channel…")}
          </option>
          {hfdlChannels(stations).map((x) => (
            <option value={String(x.khz * 1000)}>
              {x.khz} {x.names.join(", ")}
            </option>
          ))}
        </select>
        <span class="dim num" title={t("Bursts decoded / heard since tuning here")}>
          {c.decoded}/{c.bursts}
        </span>
        <button onClick={clearHfdl} title={t("Clear the list")}>
          {t("Clear")}
        </button>
        <button class="close" onClick={() => setHfdl(false)} title={t("Stop decoding")} aria-label={t("Stop decoding")}>
          ✕
        </button>
      </div>
      <div class="spots-list">
        {hfdlError.value && <p class="warn" role="alert">{hfdlError.value}</p>}
        {s && s.mode !== "usb" && <p class="dim">{t("HFDL is decoded in USB.")}</p>}
        {msgs.length === 0 ? (
          <p class="dim">
            {/* i18n: squitter = periodic announcement burst of a ground station; logon = an aircraft registering with a ground station; ACARS = aircraft messaging protocol (name kept) */}
            {t("Nothing yet: tune USB to an HFDL channel (the list above). Each ground station sends a squitter every 32 s; aircraft logons, position reports and ACARS messages come as they are sent. Only bursts whose checksums pass are listed.")}
          </p>
        ) : (
          msgs.map((m) => {
            const gs = gsName(stations, m.gs);
            return (
              <div class={`hfdl-msg ${m.dir}`}>
                <div class="rtty-head">
                  <span class="num">{hhmmss(m.utc)}</span> <strong>{where(m, gs)}</strong>
                  <span class="dim num" title={t("Rate, slot (single or double) and SNR in the symbol rate's bandwidth")}>
                    {" "}
                    · {m.bps} bps{m.slot === "D" ? " ×2" : ""} · {m.snr.toFixed(0)} dB
                  </span>
                </div>
                {m.items.length === 0 && <div class="dim">{t("(its parts didn't decode)")}</div>}
                {m.items.map((it) => (
                  <div class={`hfdl-item${it.k === "bad" ? " dim" : ""}`}>
                    <span class="hfdl-kind">{KINDS[it.k] ? t(KINDS[it.k]) : it.k}</span>
                    {it.ac !== null && it.ac !== 255 && <span class="dim num" title={t("The aircraft's ID on this ground station")}> #{it.ac}</span>}
                    {who(it) && <strong> {who(it)}</strong>}
                    {pos(it) && <span class="num"> {pos(it)}</span>} <span class="hfdl-text">{it.text}</span>
                  </div>
                ))}
              </div>
            );
          })
        )}
      </div>
    </aside>
  );
}
