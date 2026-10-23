// SPDX-License-Identifier: GPL-3.0-or-later
// The skimmers' spots: a list beside the waterfall (filter by band, mode
// and text; click a row to tune there) and the button that opens it.

import { useSignal } from "@preact/signals";
import { formatKhz } from "../fmt.ts";
import { choices, filterSpots, utcTime } from "../spots.ts";
import { pickSpot, showSpots, spots, spotsAvail, spotsOpen } from "../state.ts";

/** Rows drawn at most (the rest are a filter away). */
const ROWS = 400;

export function SpotsCtl() {
  if (!spotsAvail.value) return null;
  const open = spotsOpen.value;
  return (
    <div class="group">
      <button class={open ? "on" : ""} onClick={() => showSpots(!open)} title="Decodes of this receiver's skimmers">
        Spots
      </button>
    </div>
  );
}

export function SpotPanel() {
  const band = useSignal("");
  const mode = useSignal("");
  const text = useSignal("");
  if (!spotsAvail.value || !spotsOpen.value) return null;
  const all = spots.value;
  const list = filterSpots(all, { band: band.value, mode: mode.value, text: text.value });
  const select = (v: typeof band, opts: string[], none: string) => (
    <select value={v.value} onChange={(e) => (v.value = (e.target as HTMLSelectElement).value)}>
      <option value="">{none}</option>
      {opts.map((o) => (
        <option value={o}>{o}</option>
      ))}
    </select>
  );
  return (
    <aside class="spots">
      <div class="spots-bar">
        {select(band, choices(all, "band"), "All bands")}
        {select(mode, choices(all, "mode"), "All modes")}
        <input type="text" placeholder="Call or text" value={text.value} onInput={(e) => (text.value = (e.target as HTMLInputElement).value)} />
        <span class="dim num">{list.length}</span>
        <button class="close" onClick={() => showSpots(false)} title="Close">
          ✕
        </button>
      </div>
      <div class="spots-list">
        {list.length === 0 ? (
          <p class="dim">{all.length ? "No spots match." : "No spots yet: the skimmers decode every 15 s."}</p>
        ) : (
          <table>
            <thead>
              <tr>
                <th>UTC</th>
                <th class="r">kHz</th>
                <th class="r">dB</th>
                <th>Message</th>
              </tr>
            </thead>
            <tbody>
              {list.slice(0, ROWS).map((s) => (
                <tr onClick={() => pickSpot(s)} title={`${s.band} ${s.mode}, DT ${s.dt} s: click to tune`}>
                  <td class="dim">{utcTime(s.t)}</td>
                  <td class="r">{formatKhz(s.hz).slice(0, -2)}</td>
                  <td class="r">{s.snr}</td>
                  <td class="msg">{s.msg}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </aside>
  );
}
