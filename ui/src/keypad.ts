// SPDX-License-Identifier: GPL-3.0-or-later
// The phone's frequency keypad: what the digits typed so far mean
// (pure, unit-tested).

import { parseFreq } from "./fmt.ts";

/** Longest entry (digits and the point). */
const MAX = 10;

/** The entry after a key: a digit, ".", "⌫" (one back) or "C" (clear). */
export function keypadPress(buf: string, key: string): string {
  if (key === "⌫") return buf.slice(0, -1);
  if (key === "C") return "";
  if (key === ".") return buf.includes(".") || buf.length >= MAX ? buf : buf === "" ? "0." : `${buf}.`;
  if (/^[0-9]$/.test(key) && buf.length < MAX) return buf === "0" ? key : buf + key;
  return buf;
}

/**
 * The frequency (Hz) the entry means, or null. A bare number is kHz,
 * as typed on a keyboard; `mhz` makes it MHz (the Go-in-MHz key).
 */
export function keypadHz(buf: string, mhz: boolean): number | null {
  if (!buf || buf === ".") return null;
  return parseFreq(`${buf.replace(/\.$/, "")}${mhz ? "m" : "k"}`);
}
