// SPDX-License-Identifier: GPL-3.0-or-later
// Spots from the skimmers (and the live DX spots in the list): the list's
// rows and filters and the labels on the scale (pure, unit-tested).

import type { DxMapSpot } from "./map.ts";
import type { Spot } from "./proto.ts";

/** Spots kept in the browser. */
export const SPOTS_KEEP = 3000;

export interface SpotFilter {
  /** "" for all. */
  band: string;
  mode: string;
  /** Part of the message, any case; "" for all. */
  text: string;
}

/** Newest first, filtered. */
export function filterSpots(spots: Spot[], f: SpotFilter): Spot[] {
  const text = f.text.trim().toUpperCase();
  const out: Spot[] = [];
  for (let k = spots.length - 1; k >= 0; k--) {
    const s = spots[k];
    if (f.band && s.band !== f.band) continue;
    if (f.mode && s.mode !== f.mode) continue;
    if (text && !s.msg.toUpperCase().includes(text)) continue;
    out.push(s);
  }
  return out;
}

/** What the list shows: the skimmers' decodes, the live DX spots or both. */
export type SpotSource = "skim" | "dx" | "both";

/** A row of the list, from either source. */
export interface SpotRow {
  t: number;
  hz: number;
  band: string;
  mode: string;
  snr?: number;
  /** The message; a DX spot's station and note. */
  text: string;
  /** A skimmer's spot, to tune to as from its dial. */
  spot?: Spot;
  /** A DX spot, to tune to in its mode. */
  dx?: DxMapSpot;
}

/** The rows of `src`, newest first: the skimmers' spots (oldest first,
 * as kept) and the DX spots (the latest of each station on each band). */
export function spotRows(skim: Spot[], dx: DxMapSpot[], src: SpotSource): SpotRow[] {
  const a: SpotRow[] = [];
  if (src !== "dx")
    for (let k = skim.length - 1; k >= 0; k--) {
      const s = skim[k];
      a.push({ t: s.t, hz: s.hz, band: s.band, mode: s.mode, snr: s.snr, text: s.msg, spot: s });
    }
  if (src === "skim") return a;
  const b: SpotRow[] = dx
    .map((d) => ({ t: d.t, hz: d.hz, band: d.band, mode: d.mode, text: d.note ? `${d.dx}  ${d.note}` : d.dx, dx: d }))
    .sort((x, y) => y.t - x.t);
  if (src === "dx") return b;
  // merge, newest first
  const out: SpotRow[] = [];
  let i = 0;
  let j = 0;
  while (i < a.length || j < b.length) out.push(j >= b.length || (i < a.length && a[i].t >= b[j].t) ? a[i++] : b[j++]);
  return out;
}

/** Rows that pass the filter (the text in the message, any case). */
export function filterRows(rows: SpotRow[], f: SpotFilter): SpotRow[] {
  const text = f.text.trim().toUpperCase();
  return rows.filter((r) => (!f.band || r.band === f.band) && (!f.mode || r.mode === f.mode) && (!text || r.text.toUpperCase().includes(text)));
}

/** Add a batch, keeping the newest `keep`. */
export function addSpots(old: Spot[], batch: Spot[], keep = SPOTS_KEEP): Spot[] {
  const all = old.concat(batch);
  return all.length > keep ? all.slice(all.length - keep) : all;
}

/** The distinct values of a field, sorted (bands by frequency). */
export function choices(spots: { band: string; mode: string; hz: number }[], key: "band" | "mode"): string[] {
  const seen = new Map<string, number>();
  for (const s of spots) if (s[key] && !seen.has(s[key])) seen.set(s[key], s.hz);
  return [...seen.entries()].sort((a, b) => (key === "band" ? a[1] - b[1] : a[0].localeCompare(b[0]))).map((e) => e[0]);
}

/** "21:11:45" UTC. */
export function utcTime(t: number): string {
  return new Date(t * 1000).toISOString().slice(11, 19);
}

/** How long a spot stays on the scale: about two slots of its mode. */
export function shownFor(mode: string): number {
  return mode === "WSPR" ? 250 : mode === "FT4" ? 20 : 40;
}

/** The latest spot of each sender (or message, without one) still shown
 * at `now` (UTC seconds): what the scale shows. */
export function latest(spots: Spot[], now: number): Spot[] {
  const by = new Map<string, Spot>();
  const oldest = now - shownFor("WSPR");
  for (let k = spots.length - 1; k >= 0 && spots[k].t >= oldest; k--) {
    const s = spots[k];
    if (s.t < now - shownFor(s.mode)) continue;
    const key = `${s.mode} ${s.call ?? s.msg}`;
    if (!by.has(key)) by.set(key, s);
  }
  return [...by.values()].sort((a, b) => a.hz - b.hz);
}

export interface Placed<T> {
  item: T;
  x: number;
  /** Label row (0 nearest the scale), or -1: no room for a label. */
  row: number;
}

/** Labels of `width` px at `x` px in at most `rows` rows, left to right,
 * each in the lowest row it fits in without overlapping. */
export function placeLabels<T>(items: { item: T; x: number; width: number }[], rows: number, gap = 4): Placed<T>[] {
  const end = new Array<number>(rows).fill(-Infinity);
  return items
    .slice()
    .sort((a, b) => a.x - b.x)
    .map((it) => {
      const left = it.x - it.width / 2;
      const row = end.findIndex((e) => e + gap <= left);
      if (row >= 0) end[row] = left + it.width;
      return { item: it.item, x: it.x, row };
    });
}
