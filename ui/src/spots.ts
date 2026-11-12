// SPDX-License-Identifier: GPL-3.0-or-later
// Spots from the skimmers: the list's filters and the labels on the scale
// (pure, unit-tested).

import type { Mode, Spot } from "./proto.ts";

/** Spots kept in the browser. */
export const SPOTS_KEEP = 3000;

export interface SpotFilter {
  /** "" for all. */
  band: string;
  mode: string;
  source?: string;
  activity?: string;
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
    if (f.source && (s.source ?? "local") !== f.source) continue;
    if (f.activity === "none" && s.activities?.length) continue;
    if (f.activity && f.activity !== "none" && !s.activities?.includes(f.activity)) continue;
    if (text && ![s.call, s.msg, ...(s.refs ?? []).map((r) => r.id)].join(" ").toUpperCase().includes(text)) continue;
    out.push(s);
  }
  return out;
}

/** Add a batch, keeping the newest `keep`. */
export function addSpots(old: Spot[], batch: Spot[], keep = SPOTS_KEEP): Spot[] {
  const all = old.concat(batch);
  return all.length > keep ? all.slice(all.length - keep) : all;
}

/** The distinct values of a field, sorted (bands by frequency). */
export function choices(spots: Spot[], key: "band" | "mode"): string[] {
  const seen = new Map<string, number>();
  for (const s of spots) if (s[key] && !seen.has(s[key])) seen.set(s[key], s.dial);
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
  const oldest = now - 1800;
  for (let k = spots.length - 1; k >= 0 && spots[k].t >= oldest; k--) {
    const s = spots[k];
    if (s.t < now - (s.source === "spothole" ? 1800 : shownFor(s.mode))) continue;
    const key = s.source === "spothole" ? `external ${s.call} ${s.band} ${s.mode}` : `${s.mode} ${s.call ?? s.msg}`;
    if (!by.has(key)) by.set(key, s);
  }
  return [...by.values()].sort((a, b) => a.hz - b.hz);
}

/** A local decode uses its USB dial; an imported spot uses the reported RF. */
export function spotTarget(s: Spot): { mode: Mode; hz: number } {
  if (s.source !== "spothole") return { mode: "usb", hz: s.dial };
  const mode: Mode = s.mode === "CW" ? "cw" : s.mode === "LSB" ? "lsb" : s.mode === "AM" ? "am" : s.mode === "FM" ? "nbfm" : "usb";
  return { mode, hz: s.hz };
}

export function spotTitle(s: Spot): string {
  const detail = s.source === "spothole"
    ? `Spothole / ${s.upstream_source ?? "external"}; ${s.reported_mode === "SSB" ? `SSB → ${s.mode} by band convention; ` : ""}externally reported`
    : `${s.snr} dB, DT ${s.dt} s; locally decoded`;
  return `${utcTime(s.t)} ${s.mode} ${detail}: ${s.call ?? ""} ${s.msg}`;
}

export function activityChoices(spots: Spot[]): string[] {
  return [...new Set(["WWFF", "SOTA", "POTA", ...spots.flatMap((s) => s.activities ?? [])])].sort();
}

/** Stable chronological merge; repeated upstream snapshots cannot grow history. */
export function mergeSpots(local: Spot[], external: Spot[], now: number): Spot[] {
  const seen = new Set<string>();
  const remote = external.filter((s) => {
    if (s.t < now - 1800 || !s.id || seen.has(s.id)) return false;
    seen.add(s.id);
    return true;
  }).slice(-500);
  return [...local, ...remote].sort((a,b) => a.t - b.t || (a.id ?? "").localeCompare(b.id ?? ""));
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
