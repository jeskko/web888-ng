// SPDX-License-Identifier: GPL-3.0-or-later
// The keyboard and mouse help: `?` (or the status bar's link) shows it,
// any key or a click closes it.

import { signal } from "@preact/signals";
import { useDialog } from "../a11y.ts";
import { useEffect } from "preact/hooks";
import { N_, t } from "../i18n.ts";

export const helpOpen = signal(false);

const KEYS: [string, string][] = [
  ["← →", N_("Step the dial (shift: ×10)")],
  ["g", N_("Type a frequency (Enter tunes)")],
  ["Tab", N_("Move between the controls; the labels near the dial follow the header")],
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
  [N_("Shift+drag, shift+click"), N_("What is this? A closer look at the signals in that range (tunes there if needed)")],
  [N_("Wheel over a digit"), N_("Step that digit; click the dial to type a frequency (7074, 7.074M)")],
  [N_("Click a marker, spot or label"), N_("Tune to it, or listen along with that user")],
];

export function KeyHelp() {
  const open = helpOpen.value;
  useEffect(() => {
    if (!open) return;
    const close = (e: Event) => {
      // Tab and the modifiers move about (a screen reader's keys), not close
      if (e instanceof KeyboardEvent && ["Tab", "Shift", "Control", "Alt", "Meta", "CapsLock"].includes(e.key)) {
        window.addEventListener("keydown", close, { capture: true, once: true });
        return;
      }
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
  const box = useDialog(open, () => (helpOpen.value = false));
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
    <div ref={box} class="overlay keyhelp" role="dialog" aria-modal="true" aria-label={t("Keys")}>
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
