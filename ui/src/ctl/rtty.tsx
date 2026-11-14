// SPDX-License-Identifier: GPL-3.0-or-later
// FSK / RTTY decoded in this browser from this receiver's audio: the
// button, and beside the waterfall each two-tone signal in the passband
// with the parameters worked out from it (rate, shift, framing, polarity,
// character set) and its text; signals identified but not decodable
// (reversals, other codes) are listed as such.

import { formatKhz } from "../fmt.ts";
import { audioBand } from "../cw.ts";
import { describe, RTTY_STATIONS } from "../rtty.ts";
import { clearRtty, pickRtty, rttyError, rttyOn, rttySignals, setRtty, st } from "../state.ts";

/** Characters shown per signal (the newest). */
const SHOWN = 1500;

export function RttyCtl() {
  const on = rttyOn.value;
  return (
    <div class="group">
      <button class={on ? "on" : ""} onClick={() => setRtty(!on)} title="Decode FSK/RTTY (and NAVTEX) from this receiver's audio, in this browser: rate, shift, framing and character set found from the signal">
        RTTY
      </button>
    </div>
  );
}

export function RttyPanel() {
  if (!rttyOn.value) return null;
  const sigs = rttySignals.value;
  const s = st.value;
  const band = s && audioBand(s.mode, s.lo, s.hi, s.pitch);
  return (
    <aside class="spots rtty">
      <div class="spots-bar">
        <strong>RTTY</strong>
        <select value="" onChange={(e) => pickRtty(Number((e.target as HTMLSelectElement).value))} title="Stations to tune to (USB, the signal at 1 kHz)">
          <option value="" disabled>
            Station…
          </option>
          {RTTY_STATIONS.map((x) => (
            <option value={String(x.hz)}>{x.name}</option>
          ))}
        </select>
        <span class="dim num">{sigs.filter((x) => x.active && x.locked).length}</span>
        <button onClick={clearRtty} title="Clear the text">
          Clear
        </button>
        <button class="close" onClick={() => setRtty(false)} title="Stop decoding">
          ✕
        </button>
      </div>
      <div class="spots-list">
        {rttyError.value && <p class="warn">{rttyError.value}</p>}
        {s && !band && <p class="dim">RTTY is decoded in USB, LSB and CW.</p>}
        {sigs.length === 0 ? (
          <p class="dim">
            Nothing yet: every two-tone FSK signal in the passband is found, its rate, shift, framing (data bits, parity, stop bits), polarity and character set worked out, and decoded. A signal needs a few seconds before it locks.
          </p>
        ) : (
          sigs.map((x) => (
            <div class={`rtty-sig${x.active ? "" : " dim"}`}>
              <div class="rtty-head">
                <span class="num">{formatKhz(Math.round(x.rf)).slice(0, -2)}</span> <span>{s ? describe(x.p, s.mode) : ""}</span>
                {x.locked ? (
                  <span class="dim num">
                    {" "}
                    · {x.snr.toFixed(0)} dB{x.p.mode === "async" ? ` · ${(100 * x.fe).toFixed(0)} % framing errors` : ""}
                  </span>
                ) : (
                  <span class="dim"> · identified, not decoded</span>
                )}
              </div>
              {x.text && <pre class="rtty-text">{x.text.length > SHOWN ? "…" + x.text.slice(x.text.length - SHOWN) : x.text}</pre>}
            </div>
          ))
        )}
      </div>
    </aside>
  );
}
