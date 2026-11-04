// SPDX-License-Identifier: GPL-3.0-or-later
// The Labels control: which frequency labels to show (broadcast, utility,
// amateur, the station's own), the band ribbon, and the IARU region of
// the band plan and the amateur frequencies. Remembered in this browser.

import { useSignal } from "@preact/signals";
import { useEffect, useRef } from "preact/hooks";
import { LABEL_MAX_SPAN, LABEL_SOURCES } from "../markers.ts";
import { bandPlan, labelSources, region, ribbonOn, setLabelSources, setRegion, setRibbon } from "../state.ts";

function Options() {
  const on = labelSources.value;
  const toggle = (id: string) => setLabelSources(on.includes(id) ? on.filter((x) => x !== id) : [...on, id]);
  // "the receiver's": the region its band plan came in with
  const own = region.value === 0 && bandPlan.value ? ` (${bandPlan.value.region})` : "";
  return (
    <div class="labels-opts">
      {LABEL_SOURCES.map((s) => (
        <label key={s.id} title={s.title}>
          <input type="checkbox" checked={on.includes(s.id)} onChange={() => toggle(s.id)} /> {s.name}
        </label>
      ))}
      <label title="Band plan and service bands between the spectrum and the scale">
        <input type="checkbox" checked={ribbonOn.value} onChange={(e) => setRibbon(e.currentTarget.checked)} /> Band ribbon
      </label>
      <label title="IARU region of the band plan and the amateur frequencies">
        Region{" "}
        <select value={region.value} onChange={(e) => setRegion(Number(e.currentTarget.value))}>
          <option value={0}>receiver's{own}</option>
          <option value={1}>IARU 1</option>
          <option value={2}>IARU 2</option>
          <option value={3}>IARU 3</option>
        </select>
      </label>
      <p class="dim">Labels show at {LABEL_MAX_SPAN / 1e6} MHz wide or narrower. Click one to tune.</p>
    </div>
  );
}

/** A footer button with the options in a popup; `inline`: the options
 * themselves (the phone's More sheet). */
export function LabelsCtl({ inline = false }: { inline?: boolean }) {
  const open = useSignal(false);
  const box = useRef<HTMLDivElement>(null);
  // a click elsewhere closes the popup
  useEffect(() => {
    if (!open.value) return;
    const away = (e: PointerEvent) => {
      if (!box.current?.contains(e.target as Node)) open.value = false;
    };
    document.addEventListener("pointerdown", away);
    return () => document.removeEventListener("pointerdown", away);
  }, [open.value]);
  if (inline) return <Options />;
  return (
    <div class="group labels-ctl" ref={box}>
      <button class={open.value ? "on" : ""} onClick={() => (open.value = !open.value)} title="Frequency labels and the band ribbon">
        Labels
      </button>
      {open.value && (
        <div class="labels-pop">
          <Options />
        </div>
      )}
    </div>
  );
}
