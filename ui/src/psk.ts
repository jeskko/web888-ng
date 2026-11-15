// SPDX-License-Identifier: GPL-3.0-or-later
// PSK31/63/125 decoding in the browser (pure parts, unit-tested): the
// usual frequencies, and the skimmer's output (psk/worker.ts, the
// web888-psk crate in WebAssembly) merged into one entry per signal.

/** What the worker posts. */
export type PskEv = { k: "lock"; id: number; hz: number; baud: number } | { k: "text"; id: number; hz: number; t: number; text: string; weak?: string };

export interface PskRun {
  ev: PskEv[];
  act: { id: number; hz: number; baud: number; q: number; open: boolean }[];
  now: number;
}

/** A signal as listed. */
export interface PskSignal {
  id: number;
  /** RF frequency of its carrier (Hz). */
  rf: number;
  baud: number;
  /** Mean |cos| of its phase changes: ~1 clean, 0.64 noise. */
  q: number;
  /** Squelch open: being read. */
  open: boolean;
  text: string;
  /** Per character of `text`, "1" where it read weakly (it may have read
   * otherwise). */
  weak: string;
  /** Still in the skimmer's list. */
  active: boolean;
  /** UTC seconds of the last change. */
  last: number;
}

/** Text kept per signal (characters). */
export const PSK_TEXT = 4000;
/** A signal gone from the skimmer stays listed this long (s). */
export const PSK_LINGER = 300;

/** The usual PSK31 frequencies: USB dial (the signals at 500-2500 Hz). */
export const PSK_STATIONS: { name: string; hz: number }[] = [
  { name: "80 m 3.580 MHz", hz: 3_580_000 },
  { name: "40 m 7.040 MHz", hz: 7_040_000 },
  { name: "30 m 10.142 MHz", hz: 10_142_000 },
  { name: "20 m 14.070 MHz", hz: 14_070_000 },
  { name: "17 m 18.100 MHz", hz: 18_100_000 },
  { name: "15 m 21.070 MHz", hz: 21_070_000 },
  { name: "10 m 28.120 MHz", hz: 28_120_000 },
];

/** "PSK31", "PSK63", "PSK125". */
export function pskName(baud: number): string {
  return `PSK${Math.round(baud) === 31 ? 31 : Math.round(baud)}`;
}

/** The signals after a run. `rf` maps an audio frequency to RF; `now`
 * is UTC seconds. */
export function applyPsk(prev: PskSignal[], run: PskRun, now: number, rf: (hz: number) => number): PskSignal[] {
  const by = new Map<number, PskSignal>(prev.map((s) => [s.id, { ...s }]));
  for (const e of run.ev) {
    if (e.k === "lock") {
      if (!by.has(e.id)) by.set(e.id, { id: e.id, rf: rf(e.hz), baud: e.baud, q: 0, open: false, text: "", weak: "", active: true, last: now });
      continue;
    }
    const s = by.get(e.id);
    if (!s) continue;
    let text = s.text + e.text;
    let weak = s.weak + (e.weak?.length === e.text.length ? e.weak : "0".repeat(e.text.length));
    if (text.length > PSK_TEXT) {
      text = text.slice(text.length - PSK_TEXT);
      weak = weak.slice(weak.length - PSK_TEXT);
    }
    Object.assign(s, { text, weak, rf: rf(e.hz), last: now });
  }
  for (const s of by.values()) s.active = false;
  for (const a of run.act) {
    const s = by.get(a.id);
    if (s) Object.assign(s, { rf: rf(a.hz), baud: a.baud, q: a.q, open: a.open, active: true });
  }
  return [...by.values()].filter((s) => s.active || now - s.last < PSK_LINGER).sort((a, b) => a.rf - b.rf);
}
