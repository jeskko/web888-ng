// SPDX-License-Identifier: GPL-3.0-or-later
// CW decoded in this browser from this receiver's audio: the button that
// turns it on, and beside the waterfall one row per keyed carrier in the
// passband with its speed, level and text as it settles.

import { formatKhz } from "../fmt.ts";
import { audioBand } from "../cw.ts";
import { clearCw, cwError, cwNeural, cwOn, cwSignals, pickCw, setCw, setCwNeural, st } from "../state.ts";

/** Characters shown per signal (the newest). */
const SHOWN = 240;

export function CwCtl() {
  const on = cwOn.value;
  return (
    <div class="group">
      <button class={on ? "on" : ""} onClick={() => setCw(!on)} title="Decode CW (Morse) from this receiver's audio, in this browser: every keyed signal in the passband">
        CW
      </button>
    </div>
  );
}

export function CwPanel() {
  if (!cwOn.value) return null;
  const sigs = cwSignals.value;
  const s = st.value;
  const band = s && audioBand(s.mode, s.lo, s.hi, s.pitch);
  return (
    <aside class="spots cw">
      <div class="spots-bar">
        <strong>CW</strong>
        <span class="dim num">{sigs.filter((x) => x.active).length}</span>
        <button class={cwNeural.value ? "on" : ""} onClick={() => setCwNeural(!cwNeural.value)} title="Text from the neural decoder (trained on synthetic CW; reads weaker and rougher signals) instead of the classical one">
          Neural
        </button>
        <button onClick={clearCw} title="Clear the text">
          Clear
        </button>
        <button class="close" onClick={() => setCw(false)} title="Stop decoding">
          ✕
        </button>
      </div>
      <div class="spots-list">
        {cwError.value && <p class="warn">{cwError.value}</p>}
        {s && !band && <p class="dim">CW is decoded in CW, USB and LSB.</p>}
        {sigs.length === 0 ? (
          <p class="dim">
            Nothing yet: every keyed signal in the passband is found and decoded (9–45 WPM); text shows a few seconds behind the audio, once it has settled. A wide passband (USB) decodes several stations at once.
          </p>
        ) : (
          <table>
            <thead>
              <tr>
                <th class="r">kHz</th>
                <th class="r">WPM</th>
                <th class="r">dB</th>
                <th>Text</th>
              </tr>
            </thead>
            <tbody>
              {sigs.map((x) => (
                <tr class={x.active ? "" : "dim"} onClick={() => pickCw(Math.round(x.rf))} title={`${Math.round(x.hz)} Hz audio; click: CW mode on this signal`}>
                  <td class="r">{formatKhz(Math.round(x.rf)).slice(0, -2)}</td>
                  <td class="r num">{x.wpm > 0 ? x.wpm.toFixed(0) : "–"}</td>
                  <td class="r num">{x.snr > -90 ? x.snr.toFixed(0) : "–"}</td>
                  <td class="msg">{x.text.length > SHOWN ? "…" + x.text.slice(x.text.length - SHOWN) : x.text}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </aside>
  );
}
