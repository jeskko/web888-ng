// SPDX-License-Identifier: GPL-3.0-or-later
// What the map shows (pure, unit-tested): points with places from the
// skimmers' spots, this browser's own decodes and the live DX spots, each
// with the receiving end for its line, and the bands' colours.

import { bandOf } from "./bands.ts";
import type { DigiRow } from "./digi.ts";
import { ftxSender, GridBook, gridToLatLon, wsprSender, type LatLon } from "./geo.ts";
import type { Spot } from "./proto.ts";
import type { Tune } from "./markers.ts";

export type Layer = "skimmer" | "browser" | "dx";

export const LAYERS: { id: Layer; name: string; title: string }[] = [
  { id: "skimmer", name: "Skimmers", title: "This receiver's FT8/FT4/WSPR skimmers: senders with a known locator" },
  { id: "browser", name: "Browser", title: "What this browser decoded (FT8/FT4/WSPR button)" },
  { id: "dx", name: "DX spots", title: "Live DX cluster spots (Spothole), from the spotter to the station" },
];

/** A station on the map. */
export interface MapPoint {
  layer: Layer;
  call: string;
  pos: LatLon;
  /** Where it was heard (the receiver; a DX spot's spotter). */
  from: LatLon | null;
  /** The locator, if it came from one. */
  grid?: string;
  band: string;
  mode: string;
  /** When heard, Unix s. */
  t: number;
  hz: number;
  snr?: number;
  text: string;
  /** Click: a skimmer spot to tune to, or a frequency and mode. */
  spot?: Spot;
  tune?: { hz: number; mode: Tune };
}

/** A DX spot as `/api/map/dx` gives it. */
export interface DxMapSpot {
  t: number;
  hz: number;
  band: string;
  mode: string;
  tune: Tune;
  dx: string;
  de: string;
  note: string;
  dx_pos: LatLon | null;
  de_pos: LatLon | null;
}

/** Band colours (by wavelength, warm to cold, as maps of spots use). */
const BAND_COLOURS: Record<string, string> = {
  "2200m": "#a0a0a0",
  "630m": "#c0c0c0",
  "160m": "#7cfc00",
  "80m": "#e550e5",
  "60m": "#00008b",
  "40m": "#5959ff",
  "30m": "#62d962",
  "20m": "#f2c40c",
  "17m": "#f2f261",
  "15m": "#cca166",
  "12m": "#b22222",
  "10m": "#ff69b4",
  "6m": "#ff0000",
  "4m": "#cc0044",
  "2m": "#ff1493",
};

export function bandColour(band: string): string {
  return BAND_COLOURS[band] ?? "#d0d0d0";
}

/** The bands of a list, by frequency. */
export function bandsOf(points: MapPoint[]): string[] {
  const lowest = new Map<string, number>();
  for (const p of points) if (p.band && !lowest.has(p.band)) lowest.set(p.band, p.hz);
  return [...lowest.entries()].sort((a, b) => a[1] - b[1]).map((e) => e[0]);
}

/** The latest of each call on each band. */
function latestEach(points: MapPoint[]): MapPoint[] {
  const by = new Map<string, MapPoint>();
  for (const p of points) {
    const k = `${p.call} ${p.band}`;
    const o = by.get(k);
    if (!o || p.t >= o.t) by.set(k, p);
  }
  return [...by.values()];
}

const place = (grid: string | undefined) => (grid ? gridToLatLon(grid) : null);

/** Skimmer spots heard since `oldest` from senders with a known locator
 * (said in this spot or an earlier one: `book` learns them all). */
export function skimmerPoints(spots: Spot[], book: GridBook, rx: LatLon | null, oldest: number): MapPoint[] {
  const out: MapPoint[] = [];
  for (const s of spots) {
    const call = s.call ?? (s.mode === "WSPR" ? wsprSender(s.msg) : ftxSender(s.msg)).call;
    book.note(call, s.grid);
    if (s.t < oldest || !call) continue;
    const grid = s.grid ?? book.get(call);
    const pos = place(grid);
    if (!pos) continue;
    out.push({ layer: "skimmer", call, pos, from: rx, grid, band: s.band, mode: s.mode, t: s.t, hz: s.hz, snr: s.snr, text: s.msg, spot: s });
  }
  return latestEach(out);
}

/** This browser's decodes, the same way. */
export function browserPoints(rows: DigiRow[], book: GridBook, rx: LatLon | null, oldest: number): MapPoint[] {
  const out: MapPoint[] = [];
  for (const r of rows) {
    const who = r.mode === "wspr" ? wsprSender(r.text) : ftxSender(r.text);
    book.note(who.call, who.grid);
    if (r.t < oldest || !who.call) continue;
    const grid = who.grid ?? book.get(who.call);
    const pos = place(grid);
    if (!pos) continue;
    const b = bandOf(r.rf);
    out.push({
      layer: "browser",
      call: who.call,
      pos,
      from: rx,
      grid,
      band: b ? `${b.name}m` : "",
      mode: r.mode.toUpperCase(),
      t: r.t,
      hz: r.rf,
      snr: r.snr,
      text: r.text,
    });
  }
  return latestEach(out);
}

/** DX spots with a place since `oldest`, from their spotters. */
export function dxPoints(list: DxMapSpot[], oldest: number): MapPoint[] {
  const out: MapPoint[] = [];
  for (const s of list) {
    if (s.t < oldest || !s.dx_pos) continue;
    out.push({
      layer: "dx",
      call: s.dx,
      pos: s.dx_pos,
      from: s.de_pos,
      band: s.band,
      mode: s.mode,
      t: s.t,
      hz: s.hz,
      text: s.note,
      tune: { hz: s.hz, mode: s.tune },
    });
  }
  return latestEach(out);
}
