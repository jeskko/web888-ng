// SPDX-License-Identifier: GPL-3.0-or-later
// FT8, FT4 and WSPR decoded in this browser from this receiver's audio, on
// the frequency the user tunes (the skimmers' decoders, run here): the
// button that turns it on, and its list beside the waterfall with the
// mode, the common dial frequencies and the deeper search. Nothing
// decoded here is reported anywhere.

import { useSignal } from "@preact/signals";
import { MODES, type DigiMode } from "../digi.ts";
import { formatKhz } from "../fmt.ts";
import { utcTime } from "../spots.ts";
import { clearDigi, digiDeep, digiError, digiMode, digiOn, digiRows, digiStatus, pickDigi, setDigi, setDigiDeep, setDigiMode, st } from "../state.ts";

/** Rows drawn at most (the rest are a filter away). */
const ROWS = 400;

export function DigiCtl() {
  const on = digiOn.value;
  return (
    <div class="group">
      <button class={on ? "on" : ""} onClick={() => setDigi(!on)} title="Decode FT8, FT4 or WSPR on the frequency you tune, in this browser (nothing is reported anywhere)">
        FT8/FT4/WSPR
      </button>
    </div>
  );
}

function StatusLine() {
  const d = digiStatus.value;
  const s = st.value;
  const mode = MODES[digiMode.value];
  if (s && s.mode !== "usb") return <p class="dim">{mode.label} is decoded in USB.</p>;
  const parts: string[] = [];
  if (d.last) {
    const ms = d.last.ms < 1000 ? `${Math.round(d.last.ms)} ms` : `${(d.last.ms / 1000).toFixed(1)} s`;
    parts.push(`${utcTime(d.last.start)}: ${d.last.n} decode${d.last.n === 1 ? "" : "s"} in ${ms}${d.last.cut ? " (cut short)" : ""}`);
  }
  if (d.pending > 0) parts.push("decoding…");
  else if (d.next !== null) parts.push(`next slot ${utcTime(d.next)}`);
  else parts.push("waiting for audio");
  return (
    <p class="dim digi-status" title={d.clock ? "Slots on the receiver's clock" : "Slots on this computer's clock (the receiver's isn't known yet)"}>
      {parts.join(" · ")}
      {!d.clock && " · local clock"}
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
        <select value={mode} onChange={(e) => setDigiMode((e.target as HTMLSelectElement).value as DigiMode)} title="Mode">
          {(Object.keys(MODES) as DigiMode[]).map((k) => (
            <option value={k}>{MODES[k].label}</option>
          ))}
        </select>
        <select value={dial ? String(dial.hz) : ""} onChange={(e) => pickDigi(Number((e.target as HTMLSelectElement).value))} title={`Common ${m.label} dial frequencies (USB)`}>
          <option value="" disabled>
            Dial…
          </option>
          {m.dials.map((d) => (
            <option value={String(d.hz)}>
              {d.band} {formatKhz(d.hz).slice(0, -2)}
            </option>
          ))}
        </select>
        <label title="A deeper search: a few more decodes for two to four times the CPU">
          <input type="checkbox" checked={digiDeep.value} onChange={(e) => setDigiDeep((e.target as HTMLInputElement).checked)} /> Deep
        </label>
        <input type="text" placeholder="Call or text" value={text.value} onInput={(e) => (text.value = (e.target as HTMLInputElement).value)} />
        <span class="dim num">{rows.length}</span>
        <button onClick={clearDigi} title="Clear the list">
          Clear
        </button>
        <button class="close" onClick={() => setDigi(false)} title="Stop decoding">
          ✕
        </button>
      </div>
      <div class="spots-list">
        {digiError.value && <p class="warn">{digiError.value}</p>}
        <StatusLine />
        {rows.length === 0 ? (
          <p class="dim">
            {digiRows.value.length
              ? "No decodes match."
              : `Decodes show after each ${m.period === 120 ? "2-minute" : `${m.period} s`} slot, from the first one heard whole.${dial ? "" : ` Pick a ${m.label} dial frequency above, or tune to one in USB.`}`}
          </p>
        ) : (
          <table>
            <thead>
              <tr>
                <th>UTC</th>
                <th class="r">dB</th>
                <th class="r">DT</th>
                <th class="r">kHz</th>
                {wspr && <th class="r">Drift</th>}
                <th>Message</th>
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
