// SPDX-License-Identifier: GPL-3.0-or-later
// The keyboard and mouse help: `?` (or the status bar's link) shows it,
// any key or a click closes it.

import { signal } from "@preact/signals";
import { useEffect } from "preact/hooks";
import { N_, t } from "../i18n.ts";

export const helpOpen = signal(false);

const KEYS: [string, string][] = [
  ["← →", N_("Step the dial (shift: ×10)")],
  ["u l c", "USB, LSB, CW"],
  // i18n: SAM, SAL, SAR, SAS = synchronous AM detector (both sidebands, lower, upper, stereo)
  ["a", N_("AM, then SAM, SAL, SAR, SAS in turn")],
  ["f", "FM"],
  // i18n: I/Q = the complex baseband samples of the channel, I (in-phase) on the left audio channel and Q (quadrature) on the right
  ["i", N_("I/Q (the channel, I left and Q right)")],
  // i18n: VFO = variable frequency oscillator, one of two tuning memories (A and B) of the radio
  ["v", N_("Swap VFO A and B")],
  ["m", N_("Frequency memories")],
  ["?", N_("This help")],
];

const POINTER: [string, string][] = [
  [N_("Click"), N_("Tune there (to the step's grid)")],
  [N_("Drag, sideways swipe"), N_("Pan the waterfall")],
  [N_("Wheel, pinch"), N_("Zoom around the pointer")],
  [N_("Shift+wheel"), N_("Step the dial")],
  [N_("Wheel over a digit"), N_("Step that digit; click the dial to type a frequency (7074, 7.074M)")],
  [N_("Click a marker, spot or label"), N_("Tune to it, or listen along with that user")],
];

export function KeyHelp() {
  const open = helpOpen.value;
  useEffect(() => {
    if (!open) return;
    const close = (e: Event) => {
      e.preventDefault();
      e.stopPropagation();
      helpOpen.value = false;
    };
    // after the key that opened it
    const timer = setTimeout(() => {
      window.addEventListener("keydown", close, { capture: true, once: true });
      window.addEventListener("pointerdown", close, { capture: true, once: true });
    });
    return () => {
      clearTimeout(timer);
      window.removeEventListener("keydown", close, { capture: true });
      window.removeEventListener("pointerdown", close, { capture: true });
    };
  }, [open]);
  if (!open) return null;
  // keys in <kbd>; the pointer's gestures are words, which may wrap
  const rows = (list: [string, string][], keys: boolean) =>
    list.map(([k, what]) => (
      <tr key={k}>
        <td class={keys ? "k" : "g"}>{keys ? <kbd>{k}</kbd> : t(k)}</td>
        <td>{t(what)}</td>
      </tr>
    ));
  return (
    <div class="overlay keyhelp" role="dialog" aria-label={t("Keys")}>
      <div class="keyhelp-box">
        <h3>{t("Keys")}</h3>
        <table>
          <tbody>{rows(KEYS, true)}</tbody>
        </table>
        <h3>{t("Mouse and touch")}</h3>
        <table>
          <tbody>{rows(POINTER, false)}</tbody>
        </table>
        <p class="dim">{t("Any key or a click closes this.")}</p>
      </div>
    </div>
  );
}
