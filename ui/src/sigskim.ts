// SPDX-License-Identifier: GPL-3.0-or-later
// "Label every signal" (pure parts, unit-tested): the signal identifier
// run over channelizer bins (60 kS/s IQ, clean within ±20 kHz of their
// centre, 30 kHz apart) hopped across the waterfall's view. Each visit
// (4.1 s of a bin) finds the signals in it (crates/sigid's skim::Window)
// and names some of them; the labels are kept by RF across visits, each
// one's class probabilities averaged over its namings, and drawn on the
// spectrum (view/spectrum.ts).

import { CLASS_INFO, SURE, type SigClass } from "./sigid.ts";

/** The part of a bin labelled (Hz from its centre). */
export const BIN_HALF = 20_000;
/** The channelizer's bin spacing (Hz). */
export const BIN_STEP = 30_000;
/** A label not seen in this many visits of its range is dropped. */
export const MISSES = 2;
/** Labels kept at most (the oldest seen go first), and how long one not
 * seen is kept at all (ms: the view may come back). */
export const KEEP = 600;
export const KEEP_MS = 10 * 60_000;

export interface SkimLabel {
  id: number;
  /** RF of the centre and the edges (Hz). */
  rf: number;
  rfLo: number;
  rfHi: number;
  /** Peak over the floor (dB). */
  db: number;
  /** Class probabilities, averaged over its namings (none yet: {}). */
  probs: Partial<Record<SigClass, number>>;
  runs: number;
  /** When it was last found, last named (ms; 0: never named). */
  seen: number;
  named: number;
  /** Visits of its range since it was last found. */
  missed: number;
}

export interface Skim {
  labels: SkimLabel[];
  nextId: number;
  /** Bin centres visited (Hz) and when (ms). */
  visits: Map<number, number>;
}

export const newSkim = (): Skim => ({ labels: [], nextId: 1, visits: new Map() });

/** Overlap of two ranges over the narrower one's width. */
function overlap(a0: number, a1: number, b0: number, b1: number): number {
  return (Math.min(a1, b1) - Math.max(a0, b0)) / Math.max(1, Math.min(a1 - a0, b1 - b0));
}

/** Frequencies to ask a bin for so their bins cover lo..hi (Hz): one
 * every BIN_STEP; the server sends the bin nearest each. */
export function hopTargets(lo: number, hi: number): number[] {
  const out: number[] = [];
  for (let h = lo + BIN_STEP / 2; h - BIN_STEP / 2 < hi; h += BIN_STEP) out.push(h);
  return out;
}

/** The next frequency to ask for: the one whose bin was visited longest
 * ago (never first), nearer the dial on a tie. */
export function nextHop(sk: Skim, lo: number, hi: number, dial: number, now: number): number | null {
  const ts = hopTargets(lo, hi);
  if (!ts.length) return null;
  const last = (h: number) => {
    let t = -Infinity;
    for (const [c, at] of sk.visits) if (Math.abs(c - h) <= BIN_STEP / 2) t = Math.max(t, at);
    return t;
  };
  let best = ts[0];
  let bestAge = -1;
  for (const h of ts) {
    const age = now - last(h);
    if (age > bestAge || (age === bestAge && Math.abs(h - dial) < Math.abs(best - dial))) {
      best = h;
      bestAge = age;
    }
  }
  return best;
}

/** A visit of the bin at `centre`: the signals found in it (`regions`:
 * [lo, hi, dB] from the centre) merged with the labels known; those in
 * its range not found count a miss. Returns the ids to name, in order:
 * never named (strongest first), then the longest ago, at most `max`. */
export function visit(sk: Skim, centre: number, regions: [number, number, number][], now: number, max: number): number[] {
  const lo = centre - BIN_HALF;
  const hi = centre + BIN_HALF;
  const found = new Set<number>();
  for (const [a, b, db] of regions) {
    const rfLo = centre + a;
    const rfHi = centre + b;
    const rf = (rfLo + rfHi) / 2;
    const known = sk.labels.find((l) => !found.has(l.id) && overlap(l.rfLo, l.rfHi, rfLo, rfHi) > 0.5);
    if (known) {
      Object.assign(known, { rf, rfLo, rfHi, db, seen: now, missed: 0 });
      found.add(known.id);
    } else {
      const id = sk.nextId++;
      sk.labels.push({ id, rf, rfLo, rfHi, db, probs: {}, runs: 0, seen: now, named: 0, missed: 0 });
      found.add(id);
    }
  }
  // in range and not found: a miss; one overlapping a found one is the
  // same signal grown or split, and goes
  const now_ = sk.labels.filter((l) => found.has(l.id));
  sk.labels = sk.labels.filter((l) => {
    if (found.has(l.id) || l.rf < lo || l.rf > hi) return true;
    if (now_.some((f) => overlap(f.rfLo, f.rfHi, l.rfLo, l.rfHi) > 0.3)) return false;
    l.missed++;
    return l.missed < MISSES;
  });
  sk.labels = sk.labels.filter((l) => now - l.seen < KEEP_MS);
  if (sk.labels.length > KEEP) sk.labels = sk.labels.sort((a, b) => b.seen - a.seen).slice(0, KEEP);
  sk.labels.sort((a, b) => a.rf - b.rf);
  sk.visits.set(centre, now);
  const mine = sk.labels.filter((l) => found.has(l.id));
  return [
    ...mine.filter((l) => !l.runs).sort((a, b) => b.db - a.db),
    ...mine.filter((l) => l.runs).sort((a, b) => a.named - b.named),
  ]
    .slice(0, max)
    .map((l) => l.id);
}

/** A naming of label `id` (the identifier's likeliest classes; the rest
 * count 0): averaged into what it had, half and half. */
export function named(sk: Skim, id: number, top: [string, number][], now: number): void {
  const l = sk.labels.find((x) => x.id === id);
  if (!l) return;
  const cur: Partial<Record<SigClass, number>> = {};
  for (const [c, p] of top) if (c in CLASS_INFO) cur[c as SigClass] = p;
  if (!l.runs) l.probs = cur;
  else {
    const keys = new Set([...Object.keys(l.probs), ...Object.keys(cur)]) as Set<SigClass>;
    const avg: Partial<Record<SigClass, number>> = {};
    for (const k of keys) avg[k] = 0.5 * (l.probs[k] ?? 0) + 0.5 * (cur[k] ?? 0);
    l.probs = avg;
  }
  l.runs++;
  l.named = now;
}

/** A label to draw: its likeliest class and how sure. */
export interface SkimShown {
  id: number;
  rf: number;
  rfLo: number;
  rfHi: number;
  db: number;
  cls: SigClass;
  p: number;
  /** The second likeliest, when not sure. */
  alt?: SigClass;
  sure: boolean;
}

/** Below this the label isn't drawn at all. */
export const SHOW_MIN = 0.3;

/** The labels in lo..hi (Hz) worth drawing: named, not noise, at least
 * SHOW_MIN sure. */
export function shown(sk: Skim, lo: number, hi: number): SkimShown[] {
  const out: SkimShown[] = [];
  for (const l of sk.labels) {
    if (!l.runs || l.rfHi < lo || l.rfLo > hi) continue;
    const ranked = (Object.entries(l.probs) as [SigClass, number][]).sort((a, b) => b[1] - a[1]);
    const [best, second] = ranked;
    if (!best || best[0] === "noise" || best[1] < SHOW_MIN) continue;
    const sure = best[1] >= SURE;
    out.push({ id: l.id, rf: l.rf, rfLo: l.rfLo, rfHi: l.rfHi, db: l.db, cls: best[0], p: best[1], alt: sure ? undefined : second?.[0], sure });
  }
  return out;
}

/** A label's colour by the kind of signal: amateur digital modes blue,
 * CW yellow, voice green, FSK orange, utility and military data purple,
 * pictures pink, carriers, interference and radars grey. */
export function classColour(cls: SigClass): string {
  switch (cls) {
    case "ft8":
    case "ft4":
    case "wspr":
    case "psk31":
    case "olivia":
    case "mfsk":
    case "dominoex":
    case "hell":
      return "#7fd1ff";
    case "cw":
      return "#ffd25a";
    case "ssb":
    case "am":
      return "#7fe08f";
    case "fsk":
      return "#ffb37f";
    case "sstv":
    case "fax":
      return "#ff9ad5";
    case "carrier":
    case "interference":
    case "radar":
    case "noise":
      return "#9aa5b8";
    default:
      return "#c89bff";
  }
}
