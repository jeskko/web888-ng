// SPDX-License-Identifier: GPL-3.0-or-later
// JTTY decoded in this browser from this receiver's audio: the button that
// turns it on, and its list beside the waterfall (with the common dial
// frequencies to tune to).

import { formatKhz } from "../fmt.ts";
import { JTTY_DIALS } from "../jtty.ts";
import { utcTime } from "../spots.ts";
import { jttyError, jttyOn, jttyRows, pickJtty, setJtty, st } from "../state.ts";

/** Rows drawn at most. */
const ROWS = 300;

export function JttyCtl() {
  const on = jttyOn.value;
  return (
    <div class="group">
      <button class={on ? "on" : ""} onClick={() => setJtty(!on)} title="Decode JTTY (WSJT-X's keyboard mode) from this receiver's audio, in this browser">
        JTTY
      </button>
    </div>
  );
}

export function JttyPanel() {
  if (!jttyOn.value) return null;
  const rows = jttyRows.value;
  const s = st.value;
  const dial = JTTY_DIALS.find((d) => d.hz === s?.hz);
  return (
    <aside class="spots jtty">
      <div class="spots-bar">
        <strong>JTTY</strong>
        <select value={dial ? String(dial.hz) : ""} onChange={(e) => pickJtty(Number((e.target as HTMLSelectElement).value))} title="Common JTTY dial frequencies (USB)">
          <option value="" disabled>
            Dial…
          </option>
          {JTTY_DIALS.map((d) => (
            <option value={String(d.hz)}>
              {d.band} {formatKhz(d.hz).slice(0, -2)}
            </option>
          ))}
        </select>
        <span class="dim num">{rows.length}</span>
        <button class="close" onClick={() => setJtty(false)} title="Stop decoding">
          ✕
        </button>
      </div>
      <div class="spots-list">
        {jttyError.value && <p class="warn">{jttyError.value}</p>}
        {s && s.mode !== "usb" && <p class="dim">JTTY is decoded in USB.</p>}
        {rows.length === 0 ? (
          <p class="dim">Nothing yet: messages show when they end. Signals 200–3000 Hz above the dial; JTTY has no time slots.</p>
        ) : (
          <table>
            <thead>
              <tr>
                <th>UTC</th>
                <th class="r">kHz</th>
                <th>Message</th>
              </tr>
            </thead>
            <tbody>
              {rows
                .slice(-ROWS)
                .reverse()
                .map((r) => (
                  <tr title={`${r.hz} Hz audio, ${r.frames} frame${r.frames === 1 ? "" : "s"}`}>
                    <td class="dim">{utcTime(r.t)}</td>
                    <td class="r">{formatKhz(r.rf).slice(0, -2)}</td>
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
