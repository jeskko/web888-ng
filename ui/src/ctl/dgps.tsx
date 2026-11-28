// SPDX-License-Identifier: GPL-3.0-or-later
// Maritime DGPS beacons decoded in this browser from this receiver's
// audio: beside the waterfall, every beacon in the passband with its
// reference station, health, position (and distance from here), the
// satellites it corrects, its text messages and the beacons its almanac
// lists.

import { N_, t } from "../i18n.ts";
import { audioBand } from "../cw.ts";
import { DGPS_TYPES, dgpsDials, dgpsHealth, latLonText, type DgpsBeacon } from "../dgps.ts";
import { bearing, distanceKm, gridToLatLon, type LatLon } from "../geo.ts";
import { clearDgps, dgpsBeacons, dgpsError, dgpsOn, mapInfo, pickDgps, setDgps, st } from "../state.ts";

const HEALTH: Record<ReturnType<typeof dgpsHealth>, string> = {
  // i18n: a DGPS station's own report on its corrections
  healthy: N_("healthy"),
  "not monitored": N_("not monitored"),
  unhealthy: N_("unhealthy"),
};

/** "312 km 245°" from the receiver, if it has a locator. */
function away(rx: LatLon | null, p: LatLon): string {
  return rx ? ` · ${Math.round(distanceKm(rx, p))} km ${Math.round(bearing(rx, p))}°` : "";
}

function Beacon({ b, rx }: { b: DgpsBeacon; rx: LatLon | null }) {
  const types = Object.entries(b.types)
    .sort((x, y) => Number(x[0]) - Number(y[0]))
    .map(([k, n]) => (
      <span title={DGPS_TYPES[Number(k)] ? t(DGPS_TYPES[Number(k)]) : ""}>
        {" "}
        {k}×{n}
      </span>
    ));
  return (
    <div class={`rtty-sig${b.active ? "" : " dim"}`}>
      <div class="rtty-head">
        <span class="num">{(b.rf / 1000).toFixed(1)}</span> <span>{b.baud} Bd</span>
        {b.station !== null && (
          <span title={t("The reference station's ID (RTCM)")}>
            {" "}
            · {t("station {id}", { id: b.station })}
          </span>
        )}
        {b.health !== null && <span class={b.health === 7 ? "warn" : ""}> · {t(HEALTH[dgpsHealth(b.health)])}</span>}
        {/* i18n: words = the 30-bit RTCM data words; parity = their check bits */}
        <span class="dim num" title={t("Words passing parity, recently")}>
          {" "}
          · {Math.round(100 * b.w)} %{b.sync ? "" : ` · ${t("quiet")}`}
        </span>
      </div>
      {b.pos && (
        <div class="num" title={t("The reference station's position (RTCM type 3)")}>
          {latLonText(b.pos)}
          {away(rx, b.pos)}
        </div>
      )}
      {(b.gps.length > 0 || b.glo.length > 0) && (
        <div class="num" title={t("Satellites in its latest corrections")}>
          {b.gps.length > 0 && `GPS ${b.gps.join(" ")}`}
          {b.gps.length > 0 && b.glo.length > 0 && " · "}
          {b.glo.length > 0 && `GLONASS ${b.glo.join(" ")}`}
        </div>
      )}
      <div class="dim num" title={t("Messages by RTCM type (hover for the name), and how many were read whole")}>
        {t("messages")}
        {types} · {t("{n} of {m} whole", { n: b.complete, m: b.msgs })}
      </div>
      {b.texts.map((x) => (
        <pre class="rtty-text">{x}</pre>
      ))}
      {b.alm.length > 0 && (
        <details>
          <summary>{t("Almanac: {n} beacons", { n: b.alm.length })}</summary>
          <table class="dgps-alm num">
            {b.alm.map((a) => (
              <tr>
                <td>{a.khz.toFixed(1)}</td>
                <td>{a.baud} Bd</td>
                <td title={t("Station ID")}>#{a.id}</td>
                <td>
                  {latLonText(a.pos)}
                  {away(rx, a.pos)}
                </td>
                <td title={t("Range")}>{a.range} km</td>
              </tr>
            ))}
          </table>
        </details>
      )}
    </div>
  );
}

export function DgpsPanel() {
  if (!dgpsOn.value) return null;
  const list = dgpsBeacons.value;
  const s = st.value;
  const band = s && audioBand(s.mode, s.lo, s.hi, s.pitch);
  const info = mapInfo.value;
  const rx = info?.locator ? gridToLatLon(info.locator) : null;
  return (
    <aside class="spots dgps">
      <div class="spots-bar">
        <strong>DGPS</strong>
        <select value="" onChange={(e) => pickDgps(Number((e.target as HTMLSelectElement).value))} title={t("Tune to a part of the beacon band, 283.5-325 kHz (USB)")}>
          <option value="" disabled>
            {t("Tune…")}
          </option>
          {dgpsDials().map((d) => (
            <option value={String(d)}>
              {((d + 300) / 1000).toFixed(1)}–{((d + 3000) / 1000).toFixed(1)} kHz
            </option>
          ))}
        </select>
        <span class="dim num">{list.filter((x) => x.active).length}</span>
        <button onClick={clearDgps} title={t("Clear the beacons no longer heard")}>
          {t("Clear")}
        </button>
        <button class="close" onClick={() => setDgps(false)} title={t("Stop decoding")} aria-label={t("Stop decoding")}>
          ✕
        </button>
      </div>
      <div class="spots-list">
        {dgpsError.value && <p class="warn" role="alert">{dgpsError.value}</p>}
        {s && !band && <p class="dim">{t("DGPS is decoded in USB, LSB and CW.")}</p>}
        {list.length === 0 ? (
          <p class="dim">{t("Nothing yet: every DGPS beacon (283.5-325 kHz, MSK at 50-200 Bd) in the passband is found and its RTCM messages read: its station, health, position and satellites. A wide passband (USB, 3 kHz) reads several at once.")}</p>
        ) : (
          list.map((b) => <Beacon b={b} rx={rx} />)
        )}
      </div>
    </aside>
  );
}
