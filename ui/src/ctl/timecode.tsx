// SPDX-License-Identifier: GPL-3.0-or-later
// Time-code stations read in this browser from this receiver's audio:
// beside the waterfall, the station heard (or how to tune one), the
// current minute's seconds as they come, and the minutes read: the
// station's time, UTC, whether its checks passed and agreed with an
// earlier minute, and when it was heard by the server's clock.

import { t } from "../i18n.ts";
import { TC_STATIONS, utcText } from "../timecode.ts";
import { clearTc, pickTc, setTc, setTcWant, tcError, tcLevel, tcMinutes, tcNow, tcOn, tcSeconds, tcWant } from "../state.ts";

function carrierText(hz: number): string {
  return hz >= 1e6 ? `${hz / 1e6} MHz` : `${hz / 1e3} kHz`;
}

export function TcPanel() {
  if (!tcOn.value) return null;
  const plan = tcNow.value;
  const secs = tcSeconds.value;
  const mins = tcMinutes.value;
  const lv = tcLevel.value;
  return (
    <aside class="spots tc">
      <div class="spots-bar">
        <strong>{t("Time")}</strong>
        <select value={tcWant.value} onChange={(e) => setTcWant((e.target as HTMLSelectElement).value)} title={t("The station to read (Auto: whichever the tuning hears)")}>
          <option value="auto">{t("Auto")}</option>
          {TC_STATIONS.map((x) => (
            <option value={x.name}>{x.name}</option>
          ))}
        </select>
        <select
          value=""
          onChange={(e) => {
            const [n, c] = (e.target as HTMLSelectElement).value.split("|");
            pickTc(n, Number(c));
          }}
          title={t("Tune to a station (USB with its carrier at 1 kHz; AM on WWV)")}
        >
          <option value="" disabled>
            {t("Tune…")}
          </option>
          {TC_STATIONS.flatMap((x) =>
            x.carriers.map((c) => (
              <option value={`${x.name}|${c}`}>
                {x.name} {carrierText(c)}
              </option>
            )),
          )}
        </select>
        <button onClick={clearTc} title={t("Clear the minutes")}>
          {t("Clear")}
        </button>
        <button class="close" onClick={() => setTc(false)} title={t("Stop")}>
          ✕
        </button>
      </div>
      <div class="spots-list">
        {tcError.value && <p class="warn">{tcError.value}</p>}
        {"error" in plan ? (
          plan.error && <p class="dim">{plan.error}</p>
        ) : (
          <>
            <p class="dim" title={t(plan.station.about)}>
              {/* i18n: time code = the station's slowly keyed time signal; carrier = its unmodulated radio signal; keying = the depth of its on/off or high/low modulation */}
              {plan.station.am
                ? t("{name} on {carrier}: its 100 Hz time code", { name: plan.station.name, carrier: carrierText(plan.carrier) })
                : t("{name} on {carrier}: the carrier at {hz} Hz", { name: plan.station.name, carrier: carrierText(plan.carrier), hz: Math.round(plan.tone) })}
              {lv !== null && ` · ${lv < 6 ? t("keying {db} dB (weak: a narrow passband and slow AGC help)", { db: lv.toFixed(0) }) : t("keying {db} dB", { db: lv.toFixed(0) })}`}
            </p>
            <pre
              class="tc-secs"
              // i18n: marker = the position-marker second of the time code; hole = WWV's silent second (no tick); MSF bits A and B = the two data bits sent each second
              title={t("This minute's seconds as read (dim: unsure; M a marker, H WWV's hole, ? unread; MSF: 0-3 = bits A + 2B)")}>
              {Array.from({ length: Math.ceil(Math.max(secs.length, 1) / 10) }, (_, g) => (
                <span>
                  {g > 0 ? " " : ""}
                  {secs.slice(10 * g, 10 * g + 10).map((x) => (
                    <span class={x.q < 0.25 ? "weak" : ""}>{x.s}</span>
                  ))}
                </span>
              ))}
            </pre>
          </>
        )}
        {mins.length === 0 ? (
          <p class="dim">{t("No minute read yet: the second's timing takes ~10 s, a whole minute (from its marker) up to two more.")}</p>
        ) : (
          <table class="tc-min">
            <tbody>
              {mins.map((m) => (
                <tr class={m.confirmed ? "" : "dim"}>
                  <td class="num">{m.local}</td>
                  <td class="num">{utcText(m.utc)} UTC</td>
                  <td title={m.confirmed ? t("Its checks passed, and its time agrees with an earlier minute's") : m.ok ? t("Its checks passed (not yet agreed by another minute)") : t("A check failed: a bit read wrong")}>
                    {m.confirmed ? "✓✓" : m.ok ? "✓" : "✗"}
                  </td>
                  <td class="num" title={t("When this minute began by the server's clock, less the time it says: the way from the station, the receiver and the server's clock error")}>
                    {m.late === null ? "" : `${m.late >= 0 ? "+" : ""}${Math.round(m.late * 1000)} ms`}
                  </td>
                  <td class="dim">
                    {m.station}
                    {m.notes.length ? ` · ${m.notes.join(", ")}` : ""}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </aside>
  );
}
