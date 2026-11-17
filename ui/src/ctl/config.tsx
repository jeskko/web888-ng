// SPDX-License-Identifier: GPL-3.0-or-later
// The listener's own settings in one place: their name and whether their
// frequency is shared, the data saver, the waterfall's colours and time
// marks, and the frequency labels. All remembered in this browser (the
// name and sharing also go to the server). Opened from the header's
// Config button (`#config`); the phone's More sheet shows it inline.

import { signal } from "@preact/signals";
import { useEffect } from "preact/hooks";
import { cssGradient, PALETTE_NAMES, type Palette } from "../view/colormap.ts";
import { NAME_MAX } from "../proto.ts";
import { dataSaver, myName, palette, setDataSaver, setName, setPalette, setShare, setWfTimes, st, wfTimes } from "../state.ts";
import { LabelOptions } from "./labels.tsx";

export const configOpen = signal(location.hash === "#config");

export const PALETTE_LABELS: Record<Palette, string> = {
  web888: "Default",
  viridis: "Viridis",
  heat: "Heat",
  phosphor: "Phosphor",
  grey: "Grey",
};

/** The settings themselves (the box's body, and the phone's sheet). */
export function ConfigBody() {
  const s = st.value;
  const p = palette.value;
  return (
    <div class="user-config">
      <h4>You</h4>
      <label class="config-row" title={`Shown to the other listeners (at most ${NAME_MAX} characters)`}>
        Callsign or nickname
        <input
          type="text"
          class="name"
          maxLength={NAME_MAX}
          placeholder="callsign"
          value={myName.value}
          onChange={(e) => setName(e.currentTarget.value)}
          onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
        />
      </label>
      {s && (
        <label class="config-check">
          <input type="checkbox" class="share" checked={s.share} onChange={(e) => setShare(e.currentTarget.checked)} />
          <span>
            Share my frequency
            <span class="dim">
              {" "}
              {s.share ? "— others see it on their scale and may listen along while they wait for a receiver" : "— private: others see that you are here, not where"}
            </span>
          </span>
        </label>
      )}
      <h4>Data</h4>
      <label class="config-check">
        <input type="checkbox" class="saver" checked={dataSaver.value} onChange={(e) => setDataSaver(e.currentTarget.checked)} />
        <span>
          Data saver
          <span class="dim"> — half the waterfall lines a second and a bin for every two pixels (about a quarter of its data)</span>
        </span>
      </label>
      <h4>Waterfall</h4>
      <label class="config-row">
        Colours
        <span class="config-palette">
          <span class="swatch" style={{ backgroundImage: cssGradient(p) }} />
          <select class="palette" value={p} onChange={(e) => setPalette(e.currentTarget.value as Palette)}>
            {PALETTE_NAMES.map((n) => (
              <option key={n} value={n}>
                {PALETTE_LABELS[n]}
              </option>
            ))}
          </select>
        </span>
      </label>
      <label class="config-check">
        <input type="checkbox" class="times" checked={wfTimes.value} onChange={(e) => setWfTimes(e.currentTarget.checked)} />
        <span>
          Time marks <span class="dim">— UTC down the waterfall's left edge</span>
        </span>
      </label>
      <h4>Labels</h4>
      <LabelOptions />
    </div>
  );
}

/** The header's button. */
export function ConfigCtl() {
  return (
    <div class="group">
      <button class={configOpen.value ? "on" : ""} onClick={() => (configOpen.value = !configOpen.value)} title="Your settings: name, sharing, data saver, waterfall colours, labels">
        Config
      </button>
    </div>
  );
}

/** The settings in a box over the page; Escape or a click outside closes it. */
export function ConfigPanel() {
  const open = configOpen.value;
  useEffect(() => {
    if (!open) return;
    const key = (e: KeyboardEvent) => e.key === "Escape" && (configOpen.value = false);
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [open]);
  if (!open) return null;
  return (
    <div class="overlay keyhelp" role="dialog" aria-label="Configuration" onClick={(e) => e.target === e.currentTarget && (configOpen.value = false)}>
      <div class="keyhelp-box info-box config-box">
        <button class="info-x" onClick={() => (configOpen.value = false)} title="Close (Escape)">
          ✕
        </button>
        <h3>Configuration</h3>
        <ConfigBody />
        <p class="dim">Kept in this browser.</p>
      </div>
    </div>
  );
}
