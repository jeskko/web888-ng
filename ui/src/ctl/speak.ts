// SPDX-License-Identifier: GPL-3.0-or-later
// Read out new decoded text (CW, RTTY, PSK, JS8) through the page's polite
// live region, for screen readers; off by default (Config → Accessibility),
// since it talks a lot. The new text is gathered and spoken every few
// seconds, at most a line at a time.

import { effect, signal } from "@preact/signals";
import { announce } from "../a11y.ts";
import { formatKhz } from "../fmt.ts";
import { cwSignals, js8Msgs, pskSignals, rttySignals } from "../state.ts";

const KEY = "web888.announceDecodes";

function stored(): boolean {
  try {
    return localStorage.getItem(KEY) === "1";
  } catch {
    return false;
  }
}

/** Speak new decoded text. */
export const announceDecodes = signal(stored());

export function setAnnounceDecodes(on: boolean): void {
  announceDecodes.value = on;
  try {
    localStorage.setItem(KEY, on ? "1" : "0");
  } catch {
    // private window: for this visit only
  }
}

/** The text seen so far per decoder and signal. */
const seen = new Map<string, string>();
/** New text waiting to be spoken, per decoder and frequency. */
const pending = new Map<string, { what: string; rf: number; text: string }>();

function take(what: string, list: { id: number; rf: number; text: string }[]): void {
  for (const s of list) {
    const key = `${what}${s.id}`;
    const old = seen.get(key) ?? "";
    seen.set(key, s.text);
    if (!announceDecodes.peek() || s.text === old) continue;
    // what changed: after the common start (a decoder may rewrite its tail)
    let i = 0;
    while (i < old.length && i < s.text.length && old[i] === s.text[i]) i++;
    const add = s.text.slice(i).trim();
    if (!add) continue;
    const p = pending.get(key) ?? { what, rf: s.rf, text: "" };
    p.text = `${p.text} ${add}`.trim();
    pending.set(key, p);
  }
}

effect(() => take("CW", cwSignals.value));
effect(() => take("RTTY", rttySignals.value));
effect(() => take("PSK", pskSignals.value));
effect(() => take("JS8", js8Msgs.value));

// every 4 s: the signal with the most new text, its last 200 characters
window.setInterval(() => {
  if (!announceDecodes.value || !pending.size) {
    pending.clear();
    return;
  }
  const best = [...pending.values()].sort((a, b) => b.text.length - a.text.length)[0];
  pending.clear();
  announce(`${best.what} ${formatKhz(best.rf)}: ${best.text.slice(-200)}`);
}, 4000);
