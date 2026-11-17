// SPDX-License-Identifier: GPL-3.0-or-later
// The keyboard and mouse help: `?` (or the status bar's link) shows it,
// any key or a click closes it.

import { signal } from "@preact/signals";
import { useEffect } from "preact/hooks";

export const helpOpen = signal(false);

const KEYS: [string, string][] = [
  ["← →", "Step the dial (shift: ×10)"],
  ["u l c", "USB, LSB, CW"],
  ["a", "AM, then SAM, SAL, SAR, SAS in turn"],
  ["f", "FM"],
  ["v", "Swap VFO A and B"],
  ["m", "Frequency memories"],
  ["?", "This help"],
];

const POINTER: [string, string][] = [
  ["Click", "Tune there (to the step's grid)"],
  ["Drag, sideways swipe", "Pan the waterfall"],
  ["Wheel, pinch", "Zoom around the pointer"],
  ["Shift+wheel", "Step the dial"],
  ["Wheel over a digit", "Step that digit; click the dial to type a frequency (7074, 7.074M)"],
  ["Click a marker, spot or label", "Tune to it, or listen along with that user"],
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
    const t = setTimeout(() => {
      window.addEventListener("keydown", close, { capture: true, once: true });
      window.addEventListener("pointerdown", close, { capture: true, once: true });
    });
    return () => {
      clearTimeout(t);
      window.removeEventListener("keydown", close, { capture: true });
      window.removeEventListener("pointerdown", close, { capture: true });
    };
  }, [open]);
  if (!open) return null;
  const rows = (list: [string, string][]) =>
    list.map(([k, what]) => (
      <tr key={k}>
        <td>
          <kbd>{k}</kbd>
        </td>
        <td>{what}</td>
      </tr>
    ));
  return (
    <div class="overlay keyhelp" role="dialog" aria-label="Keys">
      <div class="keyhelp-box">
        <h3>Keys</h3>
        <table>
          <tbody>{rows(KEYS)}</tbody>
        </table>
        <h3>Mouse and touch</h3>
        <table>
          <tbody>{rows(POINTER)}</tbody>
        </table>
        <p class="dim">Any key or a click closes this.</p>
      </div>
    </div>
  );
}
