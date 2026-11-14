// SPDX-License-Identifier: GPL-3.0-or-later
// CW decoding in the browser (pure parts, unit-tested): the audio band the
// skimmer searches for a mode's passband, audio frequencies as RF, and the
// skimmer's runs (cw/worker.ts, the web888-cw crate in WebAssembly) merged
// into one row per signal with its streaming text.

import type { Mode } from "./proto.ts";

/** One decode run, as the worker reports it (times: s since its reset). */
export interface CwRun {
  ev: { id: number; hz: number; text: string; wpm: number; snr: number; t: number }[];
  act: { id: number; hz: number; wpm: number; snr: number }[];
  now: number;
}

/** A signal as listed. */
export interface CwSignal {
  id: number;
  /** Audio frequency (Hz). */
  hz: number;
  /** RF frequency (Hz). */
  rf: number;
  wpm: number;
  snr: number;
  text: string;
  /** UTC seconds of the last new text. */
  last: number;
  /** Still being decoded. */
  active: boolean;
}

/** Text kept per signal (characters). */
export const CW_TEXT = 600;
/** A signal no longer decoded stays listed this long (s). */
export const CW_LINGER = 120;

/** The audio band (Hz) holding RF signals in this mode's passband, inside
 * the filter's skirts; null where CW can't be decoded (AM, FM, IQ). */
export function audioBand(mode: Mode, lo: number, hi: number, pitch: number): { lo: number; hi: number } | null {
  let a: number;
  let b: number;
  if (mode === "usb") [a, b] = [lo, hi];
  else if (mode === "lsb") [a, b] = [-hi, -lo];
  else if (mode === "cw") [a, b] = [lo + pitch, hi + pitch];
  else return null;
  a = Math.max(a + 25, 100);
  b = Math.min(b - 25, 5500);
  if (b - a < 80) {
    const c = (a + b) / 2;
    [a, b] = [c - 40, c + 40];
  }
  return { lo: a, hi: b };
}

/** RF frequency of an audio tone at `hz` (the mode's sideband and, in CW,
 * its pitch). */
export function rfOf(mode: Mode, dial: number, pitch: number, hz: number): number {
  if (mode === "lsb") return dial - hz;
  if (mode === "cw") return dial + hz - pitch;
  return dial + hz;
}

/** The signals after a run: new text appended, levels and speeds from the
 * active list, the rest marked inactive and dropped after CW_LINGER. `now`
 * is UTC seconds, `rf` maps an audio frequency to RF. */
export function applyRun(prev: CwSignal[], run: CwRun, now: number, rf: (hz: number) => number): CwSignal[] {
  const by = new Map(prev.map((s) => [s.id, { ...s, active: false }]));
  for (const e of run.ev) {
    const s = by.get(e.id) ?? { id: e.id, hz: e.hz, rf: rf(e.hz), wpm: e.wpm, snr: e.snr, text: "", last: now, active: true };
    let text = s.text + e.text;
    if (text.length > CW_TEXT) text = text.slice(text.length - CW_TEXT);
    by.set(e.id, { ...s, text, last: now });
  }
  for (const a of run.act) {
    const s = by.get(a.id);
    if (s) by.set(a.id, { ...s, hz: a.hz, rf: rf(a.hz), wpm: a.wpm, snr: a.snr, active: true });
  }
  return [...by.values()].filter((s) => s.text.trim() !== "" && (s.active || now - s.last < CW_LINGER)).sort((a, b) => a.rf - b.rf);
}
