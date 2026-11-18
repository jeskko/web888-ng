// SPDX-License-Identifier: GPL-3.0-or-later
// FT8, FT4 and WSPR decoded in this browser from this receiver's audio, on
// the frequency the user tunes (the skimmers' decoders, run here): the
// button that turns it on, and its list beside the waterfall with the
// mode, the common dial frequencies and the deeper search. Nothing
// decoded here is reported anywhere.

import { useSignal } from "@preact/signals";
import { MODES, type DigiMode } from "../digi.ts";
import { formatKhz } from "../fmt.ts";
import { t, tn } from "../i18n.ts";
import { utcTime } from "../spots.ts";
import { clearDigi, digiDeep, digiError, digiMode, digiOn, digiRows, digiStatus, pickDigi, setDigi, setDigiDeep, setDigiMode, st } from "../state.ts";

/** Rows drawn at most (the rest are a filter away). */
const ROWS = 400;

export function DigiCtl() {
  const on = digiOn.value;
  return (
    <div class="group">
      <button class={on ? "on" : ""} aria-pressed={!!(on)} onClick={() => setDigi(!on)} title={t("Decode FT8, FT4 or WSPR on the frequency you tune, in this browser (nothing is reported anywhere)")}>
        FT8/FT4/WSPR
      </button>
    </div>
  );
}

function StatusLine() {
  const d = digiStatus.value;
  const s = st.value;
  const mode = MODES[digiMode.value];
  if (s && s.mode !== "usb") return <p class="dim">{t("{mode} is decoded in USB.", { mode: mode.label })}</p>;
  const parts: string[] = [];
  if (d.last) {
    const ms = d.last.ms < 1000 ? `${Math.round(d.last.ms)} ms` : `${(d.last.ms / 1000).toFixed(1)} s`;
    // i18n: a decode = one successfully decoded message; cut short = the decoder stopped early to keep up
    const line = d.last.cut
      ? tn("{time}: {n} decode in {ms} (cut short)", "{time}: {n} decodes in {ms} (cut short)", d.last.n, { time: utcTime(d.last.start), ms })
      : tn("{time}: {n} decode in {ms}", "{time}: {n} decodes in {ms}", d.last.n, { time: utcTime(d.last.start), ms });
    parts.push(line);
  }
  if (d.pending > 0) parts.push(t("decoding…"));
  else if (d.next !== null) parts.push(t("next slot {time}", { time: utcTime(d.next) }));
  else parts.push(t("waiting for audio"));
  return (
    <p class="dim digi-status" // i18n: slot = the fixed 15 s (FT8), 7.5 s (FT4) or 2 min (WSPR) time period in which one transmission is sent and decoded
      title={d.clock ? t("Slots on the receiver's clock") : t("Slots on this computer's clock (the receiver's isn't known yet)")}>
      {parts.join(" · ")}
      {!d.clock && ` · ${t("local clock")}`}
    </p>
  );
}

export function DigiPanel() {
  const text = useSignal("");
  if (!digiOn.value) return null;
  const mode = digiMode.value;
  const m = MODES[mode];
  const s = st.value;
  const dial = m.dials.find((d) => d.hz === s?.hz);
  const q = text.value.trim().toUpperCase();
  const rows = q ? digiRows.value.filter((r) => r.text.includes(q)) : digiRows.value;
  const wspr = mode === "wspr";
  return (
    <aside class="spots digi">
      <div class="spots-bar">
        <select value={mode} onChange={(e) => setDigiMode((e.target as HTMLSelectElement).value as DigiMode)} title={t("Mode")}>
          {(Object.keys(MODES) as DigiMode[]).map((k) => (
            <option value={k}>{MODES[k].label}</option>
          ))}
        </select>
        <select value={dial ? String(dial.hz) : ""} onChange={(e) => pickDigi(Number((e.target as HTMLSelectElement).value))} title={t("Common {mode} dial frequencies (USB)", { mode: m.label })}>
          <option value="" disabled>
            {t("Dial…")}
          </option>
          {m.dials.map((d) => (
            <option value={String(d.hz)}>
              {d.band} {formatKhz(d.hz).slice(0, -2)}
            </option>
          ))}
        </select>
        // i18n: Deep = a slower, deeper decoder search that finds a few more weak messages
        <label title={t("A deeper search: a few more decodes for two to four times the CPU")}>
          <input type="checkbox" checked={digiDeep.value} onChange={(e) => setDigiDeep((e.target as HTMLInputElement).checked)} /> {t("Deep")}
        </label>
        <input type="text" placeholder={t("Call or text")} value={text.value} onInput={(e) => (text.value = (e.target as HTMLInputElement).value)} />
        <span class="dim num">{rows.length}</span>
        <button onClick={clearDigi} title={t("Clear the list")}>
          {t("Clear")}
        </button>
        <button class="close" onClick={() => setDigi(false)} title={t("Stop decoding")} aria-label={t("Stop decoding")}>
          ✕
        </button>
      </div>
      <div class="spots-list">
        {digiError.value && <p class="warn" role="alert">{digiError.value}</p>}
        <StatusLine />
        {rows.length === 0 ? (
          <p class="dim">
            {digiRows.value.length
              ? t("No decodes match.")
              : m.period === 120
                ? dial
                  ? t("Decodes show after each 2-minute slot, from the first one heard whole.")
                  : t("Decodes show after each 2-minute slot, from the first one heard whole. Pick a {mode} dial frequency above, or tune to one in USB.", { mode: m.label })
                : dial
                  ? t("Decodes show after each {period} s slot, from the first one heard whole.", { period: m.period })
                  : t("Decodes show after each {period} s slot, from the first one heard whole. Pick a {mode} dial frequency above, or tune to one in USB.", { period: m.period, mode: m.label })}
          </p>
        ) : (
          <table>
            <thead>
              <tr>
                <th>UTC</th>
                <th class="r">dB</th>
                {/* i18n: DT = time offset of the signal from the slot start, seconds */}
                <th class="r">DT</th>
                <th class="r">kHz</th>
                {/* i18n: Drift = how far the WSPR signal's frequency drifted during the transmission, Hz per minute */}
                {wspr && <th class="r">{t("Drift")}</th>}
                <th>{t("Message")}</th>
              </tr>
            </thead>
            <tbody>
              {rows
                .slice(-ROWS)
                .reverse()
                .map((r) => (
                  <tr title={`${MODES[r.mode].label}, ${formatKhz(r.rf)}`}>
                    <td class="dim">{utcTime(r.t)}</td>
                    <td class="r">{r.snr}</td>
                    <td class="r">{r.dt.toFixed(1)}</td>
                    <td class="r">{(r.rf / 1000).toFixed(r.mode === "wspr" ? 4 : 3)}</td>
                    {wspr && <td class="r">{r.drift ?? ""}</td>}
                    <td class="msg">{r.text}</td>
                  </tr>
                ))}
            </tbody>
          </table>
        )}
      </div>
    </aside>
  );
}
