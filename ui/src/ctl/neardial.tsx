// SPDX-License-Identifier: GPL-3.0-or-later
// What the canvas shows near the dial, as text and buttons: the frequency
// labels and the other listeners closest to the dial. For screen readers
// and the keyboard; hidden until something in it has the focus.

import { t } from "../i18n.ts";
import { formatKhz } from "../fmt.ts";
import { followable, freqMarkers, openSig, pickMarker, pickUser, sigLabels, st, users, whatIs } from "../state.ts";
import { CLASS_INFO } from "../sigid.ts";

/** How many labels to list. */
const MAX_LABELS = 8;

export function NearDial() {
  const s = st.value;
  if (!s) return null;
  const labels = [...freqMarkers.value]
    .map((m) => ({ m, d: Math.abs(m.hz - s.hz) }))
    .sort((a, b) => a.d - b.d)
    .slice(0, MAX_LABELS)
    .sort((a, b) => a.m.hz - b.m.hz);
  const others = users.value.filter(followable);
  const sigs = [...sigLabels.value]
    .sort((a, b) => Math.abs(a.rf - s.hz) - Math.abs(b.rf - s.hz))
    .slice(0, MAX_LABELS)
    .sort((a, b) => a.rf - b.rf);
  return (
    <section class="near-dial" aria-label={t("Near the dial")}>
      <h2>{t("Near the dial")}</h2>
      {labels.length > 0 && (
        <ul aria-label={t("Frequency labels")}>
          {labels.map(({ m }) => (
            <li key={`${m.src}${m.hz}${m.name}`}>
              <button onClick={() => pickMarker(m)}>
                {formatKhz(m.hz)} kHz {m.name}
              </button>
            </li>
          ))}
        </ul>
      )}
      {sigs.length > 0 && (
        <ul aria-label={t("Signals named")}>
          {sigs.map((g) => {
            const info = CLASS_INFO[g.cls];
            return (
              <li key={g.id}>
                <button
                  data-sig={g.cls}
                  onClick={() => (info.opens ? openSig({ id: g.id, rf: g.rf, rfLo: g.rfLo, rfHi: g.rfHi, hz: 0, db: g.db, top: [{ cls: g.cls, p: g.p }] }, g.cls) : whatIs(g.rfLo, g.rfHi))}
                >
                  {t("{f} kHz {what} {p} %", { f: formatKhz(Math.round(g.rf)), what: g.sure ? t(info.label) : `${t(info.label)}?`, p: Math.round(100 * g.p) })}
                </button>
              </li>
            );
          })}
        </ul>
      )}
      {others.length > 0 && (
        <ul aria-label={t("Other listeners")}>
          {others.map((u) => (
            <li key={u.id}>
              <button onClick={() => pickUser(u)}>
                {t("{name} on {f} kHz {mode}", { name: u.name ?? t("a listener"), f: formatKhz(u.hz), mode: u.mode.toUpperCase() })}
              </button>
            </li>
          ))}
        </ul>
      )}
      {!labels.length && !others.length && !sigs.length && <p>{t("No labels or other listeners here.")}</p>}
    </section>
  );
}
