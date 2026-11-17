// SPDX-License-Identifier: GPL-3.0-or-later
// The label options: which frequency labels to show (broadcast, utility,
// amateur, the station's own), the band ribbon, and the IARU region of
// the band plan and the amateur frequencies. Remembered in this browser.

import { LABEL_MAX_SPAN, LABEL_SOURCES } from "../markers.ts";
import { t } from "../i18n.ts";
import { bandPlan, labelSources, region, ribbonOn, setLabelSources, setRegion, setRibbon } from "../state.ts";

/** The label options (in Config, and the phone's More sheet). */
export function LabelOptions() {
  const on = labelSources.value;
  const toggle = (id: string) => setLabelSources(on.includes(id) ? on.filter((x) => x !== id) : [...on, id]);
  // "the receiver's": the region its band plan came in with
  const own = region.value === 0 && bandPlan.value ? ` (${bandPlan.value.region})` : "";
  return (
    <div class="labels-opts">
      {LABEL_SOURCES.map((s) => (
        <label key={s.id} title={t(s.title)}>
          <input type="checkbox" checked={on.includes(s.id)} onChange={() => toggle(s.id)} /> {t(s.name)}
        </label>
      ))}
      <label title={t("Band plan and service bands between the spectrum and the scale")}>
        <input type="checkbox" checked={ribbonOn.value} onChange={(e) => setRibbon(e.currentTarget.checked)} /> {t("Band ribbon")}
      </label>
      <label title={t("IARU region of the band plan and the amateur frequencies")}>
        {t("Region")}{" "}
        <select value={region.value} onChange={(e) => setRegion(Number(e.currentTarget.value))}>
          <option value={0}>
            {t("receiver's")}
            {own}
          </option>
          <option value={1}>IARU 1</option>
          <option value={2}>IARU 2</option>
          <option value={3}>IARU 3</option>
        </select>
      </label>
      <p class="dim">{t("Labels show at {n} MHz wide or narrower. Click one to tune.", { n: LABEL_MAX_SPAN / 1e6 })}</p>
    </div>
  );
}
