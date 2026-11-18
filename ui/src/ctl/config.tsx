// SPDX-License-Identifier: GPL-3.0-or-later
// The listener's own settings in one place: the language, their name and
// whether their frequency is shared, the data saver, the waterfall's
// colours and time marks, and the frequency labels. All remembered in
// this browser (the name and sharing also go to the server). Opened from
// the header's Config button (`#config`); the phone's More sheet shows it
// inline.

import { signal } from "@preact/signals";
import { announceDecodes, setAnnounceDecodes } from "./speak.ts";
import { useDialog } from "../a11y.ts";
import { useEffect } from "preact/hooks";
import { cssGradient, PALETTE_NAMES, type Palette } from "../view/colormap.ts";
import { NAME_MAX } from "../proto.ts";
import { dataSaver, myName, palette, setDataSaver, setName, setPalette, setShare, setWfTimes, st, wfTimes } from "../state.ts";
import { languages, langPref, N_, setLanguage, t } from "../i18n.ts";
import { LabelOptions } from "./labels.tsx";

export const configOpen = signal(location.hash === "#config");

// i18n: names of waterfall colour palettes
export const PALETTE_LABELS: Record<Palette, string> = {
  web888: N_("Default"),
  // i18n: the default colours, but brighter at every step for stronger signals (also with colour blindness)
  even: N_("Even brightness (colour-blind safe)"),
  // i18n: Viridis: a perceptually uniform colour scale that people with colour blindness see as well
  viridis: N_("Viridis (colour-blind safe)"),
  heat: N_("Heat"),
  phosphor: N_("Phosphor"),
  grey: N_("Grey"),
};

/** The settings themselves (the box's body, and the phone's sheet). */
export function ConfigBody() {
  const s = st.value;
  const p = palette.value;
  return (
    <div class="user-config">
      <label class="config-row">
        {t("Language")}
        <select class="lang" value={langPref.value} onChange={(e) => void setLanguage(e.currentTarget.value)}>
          <option value="">{t("Automatic (the browser's)")}</option>
          {languages.value.map((l) => (
            <option key={l.code} value={l.code} lang={l.code}>
              {l.name}
            </option>
          ))}
        </select>
      </label>
      <h4>{t("You")}</h4>
      <label class="config-row" title={t("Shown to the other listeners (at most {n} characters)", { n: NAME_MAX })}>
        {t("Callsign or nickname")}
        <input
          type="text"
          class="name"
          maxLength={NAME_MAX}
          placeholder={t("callsign")}
          value={myName.value}
          onChange={(e) => setName(e.currentTarget.value)}
          onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
        />
      </label>
      {s && (
        <label class="config-check">
          <input type="checkbox" class="share" checked={s.share} onChange={(e) => setShare(e.currentTarget.checked)} />
          <span>
            {t("Share my frequency")}
            <span class="dim">
              {" — "}
              {s.share ? t("others see it on their scale and may listen along while they wait for a receiver") : t("private: others see that you are here, not where")}
            </span>
          </span>
        </label>
      )}
      <h4>{t("Data")}</h4>
      <label class="config-check">
        <input type="checkbox" class="saver" checked={dataSaver.value} onChange={(e) => setDataSaver(e.currentTarget.checked)} />
        <span>
          {t("Data saver")}
          <span class="dim">
            {" — "}
            {t("half the waterfall lines a second and a bin for every two pixels (about a quarter of its data)")}
          </span>
        </span>
      </label>
      <h4>{t("Accessibility")}</h4>
      <label class="config-check">
        <input type="checkbox" class="speak" checked={announceDecodes.value} onChange={(e) => setAnnounceDecodes(e.currentTarget.checked)} />
        <span>
          {t("Read out decoded text")}
          <span class="dim">
            {" — "}
            {t("a screen reader speaks new CW, RTTY, PSK and JS8 text every few seconds")}
          </span>
        </span>
      </label>
      <h4>{t("Waterfall")}</h4>
      <label class="config-row">
        {t("Colours")}
        <span class="config-palette">
          <span class="swatch" style={{ backgroundImage: cssGradient(p) }} />
          <select class="palette" value={p} onChange={(e) => setPalette(e.currentTarget.value as Palette)}>
            {PALETTE_NAMES.map((n) => (
              <option key={n} value={n}>
                {t(PALETTE_LABELS[n])}
              </option>
            ))}
          </select>
        </span>
      </label>
      <label class="config-check">
        <input type="checkbox" class="times" checked={wfTimes.value} onChange={(e) => setWfTimes(e.currentTarget.checked)} />
        <span>
          {t("Time marks")}
          <span class="dim">
            {" — "}
            {t("UTC down the waterfall's left edge")}
          </span>
        </span>
      </label>
      <h4>{t("Labels")}</h4>
      <LabelOptions />
    </div>
  );
}

/** The header's button. */
export function ConfigCtl() {
  return (
    <div class="group">
      <button class={configOpen.value ? "on" : ""} aria-expanded={configOpen.value} aria-haspopup="dialog" onClick={() => (configOpen.value = !configOpen.value)} title={t("Your settings: language, name, sharing, data saver, waterfall colours, labels")}>
        {t("Config")}
      </button>
    </div>
  );
}

/** The settings in a box over the page; Escape or a click outside closes it. */
export function ConfigPanel() {
  const open = configOpen.value;
  const box = useDialog(open, () => (configOpen.value = false));
  if (!open) return null;
  return (
    <div ref={box} class="overlay keyhelp" role="dialog" aria-modal="true" aria-label={t("Configuration")} onClick={(e) => e.target === e.currentTarget && (configOpen.value = false)}>
      <div class="keyhelp-box info-box config-box">
        <button class="info-x" onClick={() => (configOpen.value = false)} title={t("Close (Escape)")} aria-label={t("Close (Escape)")}>
          ✕
        </button>
        <h3>{t("Configuration")}</h3>
        <ConfigBody />
        <p class="dim">{t("Kept in this browser.")}</p>
      </div>
    </div>
  );
}
