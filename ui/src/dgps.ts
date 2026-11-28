// SPDX-License-Identifier: GPL-3.0-or-later
// Maritime DGPS beacons decoded in the browser (pure parts, unit-tested):
// the skimmer's output (dgps/worker.ts, the web888-dgps crate in
// WebAssembly) merged into one entry per beacon, and what its RTCM
// messages say.

import type { LatLon } from "./geo.ts";
import { N_ } from "./i18n.ts";

/** An almanac entry as the worker posts it: [lat, lon, range km, kHz,
 * health, station ID, baud]. */
export type DgpsAlmRow = [number, number, number, number, number, number, number];

/** What the worker posts. */
export type DgpsEv =
  | { k: "lock"; id: number; hz: number; baud: number }
  | {
      k: "msg";
      id: number;
      hz: number;
      t: number;
      type: number;
      sid: number;
      z: number;
      seq: number;
      health: number;
      ok: number;
      n: number;
      gps?: number[];
      glo?: number[];
      pos?: [number, number, number];
      alm?: DgpsAlmRow[];
      text?: string;
    };

export interface DgpsRun {
  ev: DgpsEv[];
  act: { id: number; hz: number; baud: number; q: number; w: number; sync: boolean }[];
  now: number;
}

/** A beacon in another's almanac (type 7). */
export interface DgpsAlm {
  pos: LatLon;
  /** Range, km. */
  range: number;
  khz: number;
  /** 0 operating, 1 no integrity monitor, 2 no information, 3 don't use. */
  health: number;
  id: number;
  baud: number;
}

/** A beacon as listed. */
export interface DgpsBeacon {
  id: number;
  /** RF frequency of its carrier (Hz). */
  rf: number;
  baud: number;
  /** Average phase turn over 90°: ~1 clean. */
  q: number;
  /** Share of its words passing parity. */
  w: number;
  /** In step with its messages. */
  sync: boolean;
  /** Reference station ID, health (0-7), from the last header. */
  station: number | null;
  health: number | null;
  /** Messages by type, and how many read whole. */
  types: Record<number, number>;
  msgs: number;
  complete: number;
  /** Satellites in the last corrections. */
  gps: number[];
  glo: number[];
  /** The reference station (type 3). */
  pos: LatLon | null;
  /** Text messages (16, 36), newest last. */
  texts: string[];
  alm: DgpsAlm[];
  /** Still in the skimmer's list. */
  active: boolean;
  /** UTC seconds of the last change. */
  last: number;
}

/** A beacon gone from the skimmer stays listed this long (s). */
export const DGPS_LINGER = 600;
/** Text messages kept per beacon. */
export const DGPS_TEXTS = 5;

/** RTCM message types, for titles. */
// i18n: RTCM SC-104 message types (DGPS); GPS and GLONASS stay in English
export const DGPS_TYPES: Record<number, string> = {
  1: N_("GPS corrections"),
  3: N_("reference station position"),
  5: N_("GPS constellation health"),
  6: N_("null frame"),
  7: N_("beacon almanac"),
  9: N_("GPS partial corrections"),
  16: N_("text"),
  27: N_("extended almanac"),
  31: N_("GLONASS corrections"),
  34: N_("GLONASS partial corrections"),
  36: N_("GLONASS text"),
};

/** "healthy", "not monitored" or "unhealthy" (header health 0-5, 6, 7). */
export function dgpsHealth(h: number): "healthy" | "not monitored" | "unhealthy" {
  return h <= 5 ? "healthy" : h === 6 ? "not monitored" : "unhealthy";
}

/** Dials to tune for the beacon band (283.5-325 kHz) in USB with a
 * 300-3000 Hz passband, 2.7 kHz apart. */
export function dgpsDials(): number[] {
  const out: number[] = [];
  for (let d = 283_200; d + 300 < 325_000; d += 2_700) out.push(d);
  return out;
}

/** The beacons after a run. `rf` maps an audio frequency to RF; `now`
 * is UTC seconds. */
export function applyDgps(prev: DgpsBeacon[], run: DgpsRun, now: number, rf: (hz: number) => number): DgpsBeacon[] {
  const by = new Map<number, DgpsBeacon>(prev.map((s) => [s.id, { ...s }]));
  for (const e of run.ev) {
    if (e.k === "lock") {
      if (!by.has(e.id))
        by.set(e.id, {
          id: e.id,
          rf: rf(e.hz),
          baud: e.baud,
          q: 0,
          w: 0,
          sync: true,
          station: null,
          health: null,
          types: {},
          msgs: 0,
          complete: 0,
          gps: [],
          glo: [],
          pos: null,
          texts: [],
          alm: [],
          active: true,
          last: now,
        });
      continue;
    }
    const s = by.get(e.id);
    if (!s) continue;
    s.rf = rf(e.hz);
    s.last = now;
    s.station = e.sid;
    s.health = e.health;
    s.types = { ...s.types, [e.type]: (s.types[e.type] ?? 0) + 1 };
    s.msgs++;
    if (e.ok === e.n) s.complete++;
    if (e.gps) s.gps = e.gps;
    if (e.glo) s.glo = e.glo;
    if (e.pos) s.pos = [e.pos[0], e.pos[1]];
    if (e.text !== undefined && e.text.trim() && s.texts[s.texts.length - 1] !== e.text) s.texts = [...s.texts, e.text].slice(-DGPS_TEXTS);
    if (e.alm) {
      const alm = [...s.alm];
      for (const [lat, lon, range, khz, health, id, baud] of e.alm) {
        // an empty entry (all zero) fills a message
        if (!id && !lat && !lon) continue;
        const a: DgpsAlm = { pos: [lat, lon], range, khz, health, id, baud };
        const i = alm.findIndex((x) => x.id === id && x.khz === khz);
        if (i >= 0) alm[i] = a;
        else alm.push(a);
      }
      s.alm = alm.sort((a, b) => a.khz - b.khz);
    }
  }
  for (const s of by.values()) s.active = false;
  for (const a of run.act) {
    const s = by.get(a.id);
    if (s) Object.assign(s, { rf: rf(a.hz), baud: a.baud, q: a.q, w: a.w, sync: a.sync, active: true });
  }
  // a beacon found again (after a fade) replaces the old entry
  const list = [...by.values()].filter((s) => s.active || now - s.last < DGPS_LINGER);
  const keep = list.filter((s) => s.active || !list.some((o) => o !== s && o.active && Math.abs(o.rf - s.rf) < 0.4 * s.baud));
  return keep.sort((a, b) => a.rf - b.rf);
}

/** "59.97° N 24.38° E". */
export function latLonText([lat, lon]: LatLon): string {
  return `${Math.abs(lat).toFixed(2)}° ${lat >= 0 ? "N" : "S"} ${Math.abs(lon).toFixed(2)}° ${lon >= 0 ? "E" : "W"}`;
}
