// SPDX-License-Identifier: GPL-3.0-or-later
// The spot list beside the waterfall: the skimmers' decodes, the live DX
// spots or both (filter by band, mode and text; click a row to tune
// there) and the button that opens it.

import { useSignal } from "@preact/signals";
import { formatKhz } from "../fmt.ts";
import { choices, filterRows, spotRows, utcTime, type SpotRow, type SpotSource } from "../spots.ts";
import { dxAvail, mapDx, pickDxSpot, pickSpot, setSpotsSrc, showSpots, spots, spotsAvail, spotsOpen, spotsSrcNow } from "../state.ts";

/** Rows drawn at most (the rest are a filter away). */
const ROWS = 400;

const SOURCE_NAMES: [SpotSource, string][] = [
  ["skim", "Skimmers"],
  ["dx", "DX spots"],
  ["both", "Both"],
];

export function SpotsCtl() {
  if (!spotsAvail.value && !dxAvail.value) return null;
  const open = spotsOpen.value;
  const what = spotsAvail.value ? (dxAvail.value ? "Decodes of this receiver's skimmers and live DX spots" : "Decodes of this receiver's skimmers") : "Live DX spots";
  return (
    <div class="group">
      <button class={open ? "on" : ""} onClick={() => showSpots(!open)} title={what}>
        Spots
      </button>
    </div>
  );
}

const pick = (r: SpotRow) => (r.spot ? pickSpot(r.spot) : r.dx && pickDxSpot(r.dx));

const rowTitle = (r: SpotRow) =>
  r.spot ? `${r.band} ${r.mode}, DT ${r.spot.dt} s: click to tune` : `${r.band} ${r.mode} DX spot${r.dx?.de ? ` by ${r.dx.de}` : ""}: click to tune`;

export function SpotPanel() {
  const band = useSignal("");
  const mode = useSignal("");
  const text = useSignal("");
  if ((!spotsAvail.value && !dxAvail.value) || !spotsOpen.value) return null;
  const src = spotsSrcNow.value;
  const all = spotRows(spots.value, mapDx.value, src);
  const list = filterRows(all, { band: band.value, mode: mode.value, text: text.value });
  const select = (v: typeof band, opts: string[], none: string) => (
    <select value={v.value} onChange={(e) => (v.value = (e.target as HTMLSelectElement).value)}>
      <option value="">{none}</option>
      {opts.map((o) => (
        <option value={o}>{o}</option>
      ))}
    </select>
  );
  const empty = all.length
    ? "No spots match."
    : src === "skim"
      ? "No spots yet: the skimmers decode every 15 s."
      : src === "dx"
        ? "No DX spots in this receiver's range yet."
        : "No spots yet.";
  return (
    <aside class="spots">
      <div class="spots-bar">
        {spotsAvail.value && dxAvail.value && (
          <select value={src} onChange={(e) => setSpotsSrc((e.target as HTMLSelectElement).value as SpotSource)} title="Which spots the list shows">
            {SOURCE_NAMES.map(([id, name]) => (
              <option value={id}>{name}</option>
            ))}
          </select>
        )}
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
          <p class="dim">{empty}</p>
        ) : (
          <table>
            <thead>
              <tr>
                <th>UTC</th>
                <th class="r">kHz</th>
                <th class="r">{src === "dx" ? "Mode" : "dB"}</th>
                <th>{src === "dx" ? "Station" : "Message"}</th>
              </tr>
            </thead>
            <tbody>
              {list.slice(0, ROWS).map((r) => (
                <tr class={r.dx ? "dx" : ""} onClick={() => pick(r)} title={rowTitle(r)}>
                  <td class="dim">{utcTime(r.t)}</td>
                  <td class="r">{formatKhz(r.hz).slice(0, -2)}</td>
                  <td class="r">{r.snr ?? <span class="dx-tag">{r.mode}</span>}</td>
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
