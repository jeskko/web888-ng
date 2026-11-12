// SPDX-License-Identifier: GPL-3.0-or-later
// Local decodes and externally reported spots share filters and tuning controls.
import { useSignal } from "@preact/signals";
import { isMobile } from "../mobile.ts";
import { formatKhz } from "../fmt.ts";
import { activityChoices, choices, spotTitle, utcTime } from "../spots.ts";
import { allSpots, externalSpots, pickSpot, showSpots, spotFilter, spotsAvail, spotsOpen, visibleSpots } from "../state.ts";

const ROWS = 400;
export function SpotsCtl() {
  if (!spotsAvail.value) return null;
  return <div class="group"><button class={spotsOpen.value ? "on" : ""} onClick={() => showSpots(!spotsOpen.value)} title="Local decodes and external spots">Spots</button></div>;
}

export function SpotPanel() {
  const filtersOpen = useSignal(false);
  if (!spotsAvail.value || !spotsOpen.value) return null;
  const all = allSpots.value;
  const list = [...visibleSpots.value].reverse();
  const f = spotFilter.value;
  const update = (key: keyof typeof f, value: string) => spotFilter.value = { ...spotFilter.value, [key]: value };
  const select = (key: keyof typeof f, opts: string[], none: string, label: string) => (
    <select aria-label={label} value={f[key]} onChange={(e) => update(key, (e.target as HTMLSelectElement).value)}>
      <option value="">{none}</option>
      {opts.map((o) => <option value={o}>{o === "local" ? "Local" : o === "spothole" ? "Spothole" : o === "none" ? "No activity" : o}</option>)}
    </select>
  );
  const feed = externalSpots.value;
  return (
    <aside class="spots" aria-label="Spots">
      {isMobile.value && <div class="spots-bar spots-mobile-heading">
        <strong>Spots</strong><span class="dim num">{list.length}</span>
        <button aria-expanded={filtersOpen.value} aria-controls="spots-filters" onClick={() => filtersOpen.value = !filtersOpen.value}>Filters</button>
        <button class="close" onClick={() => showSpots(false)} title="Close spots">✕</button>
      </div>}
      {(!isMobile.value || filtersOpen.value) && <div class="spots-bar" id="spots-filters">
        {select("band", choices(all, "band"), "All bands", "Spot band")}
        {select("mode", choices(all, "mode"), "All modes", "Spot mode")}
        {select("source", ["local", "spothole"], "All sources", "Spot source")}
        {select("activity", ["none", ...activityChoices(all)], "All activities", "Spot activity")}
        <input aria-label="Search spots" type="text" placeholder="Call, text or reference" value={f.text} onInput={(e) => update("text", (e.target as HTMLInputElement).value)} />
        {!isMobile.value && <><span class="dim num">{list.length}</span><button class="close" onClick={() => showSpots(false)} title="Close spots">✕</button></>}
      </div>}
      {feed.enabled && (!isMobile.value || feed.state === "stale" || feed.state === "unavailable") && <div class="spots-status dim" role="status">
        Spothole: {feed.state}{feed.last_success != null ? ` · updated ${utcTime(feed.last_success)} UTC` : ""}
        {feed.error ? ` · ${feed.error}` : ""}
      </div>}
      <div class="spots-list">
        {list.length === 0 ? <p class="dim">{all.length ? "No spots match." : feed.enabled ? (feed.state === "loading" ? "Loading Spothole spots…" : "No recent spots. Local decodes appear when skimmers are running.") : "No spots yet: the skimmers decode every 15 s."}</p> : isMobile.value ? (
          <ul class="spot-cards">{list.slice(0, ROWS).map((s) => (
            <li class="spot-record">
              <button class="spot-card spot-tune" title={spotTitle(s)} aria-label={`Tune ${s.call ?? s.msg} ${s.mode} ${s.hz} Hz`} onClick={() => pickSpot(s)}>
                <strong class="spot-row-call">{s.call ?? s.mode}</strong>
                <span class="spot-row-freq">{formatKhz(s.hz).slice(0, -2)}</span>
                <b class="spot-row-mode">{s.mode}</b>
                <span class="spot-row-activity">{s.activities?.join(" / ") || "—"}</span>
              </button>
            </li>
          ))}</ul>
        ) : (
          <table>
            <thead><tr><th>UTC</th><th class="r">kHz</th><th>Mode</th><th class="r">dB</th><th>Message / activity</th></tr></thead>
            <tbody>{list.slice(0, ROWS).map((s) => (
              <tr class="spot-record" onClick={() => pickSpot(s)} title={`${spotTitle(s)}; click to tune`}>
                <td class="dim">{utcTime(s.t)}</td>
                <td class="r"><button class="spot-tune" aria-label={`Tune ${s.call ?? s.msg} ${s.mode} ${s.hz} Hz`} onClick={(e) => { e.stopPropagation(); pickSpot(s); }}>{formatKhz(s.hz).slice(0, -2)}</button></td>
                <td>{s.mode}</td><td class="r">{s.snr ?? "—"}</td>
                <td class="msg">{s.source === "spothole" ? `${s.call} ${s.msg === s.call ? "" : s.msg}` : s.msg}
                  <span class="spot-meta dim">{s.source === "spothole" ? `Spothole${s.upstream_source ? ` / ${s.upstream_source}` : ""}` : "Local"}
                    {s.activities?.length ? ` · ${s.activities.join(" / ")}` : ""}
                    {s.refs?.length ? ` · ${s.refs.map((r) => r.id).join(", ")}` : ""}
                  </span>
                </td>
              </tr>
            ))}</tbody>
          </table>
        )}
      </div>
    </aside>
  );
}
