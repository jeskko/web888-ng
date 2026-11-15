// SPDX-License-Identifier: GPL-3.0-or-later
// JS8 decoded in this browser from this receiver's audio (USB): beside
// the waterfall, the messages heard, newest first (frames from one
// frequency joined, as JS8Call's band activity shows them), and the
// stations heard.

import { formatKhz } from "../fmt.ts";
import { heard, JS8_DIALS, type Js8Msg } from "../js8.ts";
import { clearJs8, js8Error, js8Msgs, js8On, js8Status, pickJs8, setJs8, st } from "../state.ts";

const hhmmss = (utc: number) => new Date(utc * 1000).toISOString().slice(11, 19);

function Line({ m }: { m: Js8Msg }) {
  return (
    <div class={`js8-msg${m.done ? "" : " open"}`}>
      <div class="rtty-head">
        <span class="num">{hhmmss(m.utc)}</span> <span class="num">{formatKhz(Math.round(m.rf)).slice(0, -2)}</span>
        <span class="dim num">
          {" "}
          · {m.sub} · {m.snr} dB
        </span>
      </div>
      <pre class="rtty-text">
        {m.partial ? "… " : ""}
        {m.text.trim()}
        {m.done ? " ♢" : " …"}
      </pre>
    </div>
  );
}

export function Js8Panel() {
  if (!js8On.value) return null;
  const msgs = js8Msgs.value;
  const s = st.value;
  const stations = heard(msgs).slice(0, 30);
  const status = js8Status.value;
  return (
    <aside class="spots js8">
      <div class="spots-bar">
        <strong>JS8</strong>
        <select value="" onChange={(e) => pickJs8(Number((e.target as HTMLSelectElement).value))} title="JS8Call's usual frequencies (USB)">
          <option value="" disabled>
            Band…
          </option>
          {JS8_DIALS.map((x) => (
            <option value={String(x.hz)}>{x.name}</option>
          ))}
        </select>
        <span class="dim num" title={`Frames decoded, in ${status.periods} periods of the four submodes${status.pending ? `; ${status.pending} waiting` : ""}`}>
          {status.frames}
          {status.pending > 0 ? "…" : ""}
        </span>
        <button onClick={clearJs8} title="Clear the messages">
          Clear
        </button>
        <button class="close" onClick={() => setJs8(false)} title="Stop decoding">
          ✕
        </button>
      </div>
      <div class="spots-list">
        {js8Error.value && <p class="warn">{js8Error.value}</p>}
        {s && s.mode !== "usb" && <p class="dim">JS8 is decoded in USB.</p>}
        {!status.clock && <p class="dim">Using this computer's clock: JS8 needs it within a second or so of UTC.</p>}
        {stations.length > 0 && (
          <p class="js8-heard">
            <span class="dim">Heard: </span>
            {stations.map((h) => (
              <span class="num" title={`${hhmmss(h.utc)}, ${h.snr} dB`}>
                {h.call}{" "}
              </span>
            ))}
          </p>
        )}
        {msgs.length === 0 ? (
          <p class="dim">Nothing yet: JS8's Normal, Fast, Turbo and Slow periods are each decoded as they end, every signal in the passband. Messages of several frames are joined per frequency.</p>
        ) : (
          msgs.map((m) => <Line m={m} />)
        )}
      </div>
    </aside>
  );
}
