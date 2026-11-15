// SPDX-License-Identifier: GPL-3.0-or-later
// HFDL (aircraft HF data link) decoding in the browser (pure parts,
// unit-tested): the decoder's output (hfdl/worker.ts, the web888-hfdl
// crate in WebAssembly) kept as a log of messages, the ground stations'
// channels, and where in the audio a channel's bursts sit.

/** One part of a burst, as the worker posts it. */
export interface HfdlItem {
  /** The aircraft's ID on its ground station (none: a squitter). */
  ac: number | null;
  /** "squitter", "acars", "position", "frequencies", "logon", "logon
   * confirm", "logoff", "logon denied", "system table", "data", "bad",
   * "other". */
  k: string;
  text: string;
  flight: string;
  reg: string;
  /** ICAO address, hex. */
  icao: string | null;
  lat: number | null;
  lon: number | null;
}

/** A burst; `dir` "" when no CRC passed. */
export interface HfdlEv {
  t: number;
  bps: number;
  slot: "S" | "D";
  f: number;
  snr: number;
  dir: "up" | "down" | "squitter" | "";
  gs: number | null;
  items: HfdlItem[];
}

export interface HfdlRun {
  ev: HfdlEv[];
  bursts: number;
  decoded: number;
  now: number;
}

export interface HfdlStation {
  id: number;
  name: string;
  lat: number;
  lon: number;
  /** Listed frequencies (kHz): the USB dial; the bursts are 1440 Hz above. */
  f: number[];
}

/** A decoded burst as listed. */
export interface HfdlMsg extends HfdlEv {
  /** UTC seconds. */
  utc: number;
  /** RF (Hz) of the channel's listed frequency. */
  rf: number;
}

/** The bursts' centre above the listed frequency (Hz). */
export const HFDL_CENTRE = 1440;
/** Messages kept. */
export const HFDL_KEEP = 500;

/** Messages after a run, newest first: decoded bursts only (the rest are
 * counted). `now` is UTC seconds, `rf` the channel's listed frequency. */
export function applyHfdl(prev: HfdlMsg[], run: HfdlRun, now: number, rf: number): HfdlMsg[] {
  const add = run.ev.filter((e) => e.dir !== "").map((e) => ({ ...e, utc: now - (run.now - e.t), rf }));
  if (add.length === 0) return prev;
  return [...add.reverse(), ...prev].slice(0, HFDL_KEEP);
}

/** "Shannon" from "Shannon, Ireland". */
export function shortName(name: string): string {
  return name.split(",")[0];
}

/** The station's short name, or "GS n". */
export function gsName(stations: HfdlStation[], id: number | null): string {
  if (id === null) return "";
  const s = stations.find((x) => x.id === id);
  return s ? shortName(s.name) : `GS ${id}`;
}

/** Every listed channel, by frequency: [kHz, station names]. */
export function hfdlChannels(stations: HfdlStation[]): { khz: number; names: string[] }[] {
  const by = new Map<number, string[]>();
  for (const s of stations) for (const f of s.f) by.set(f, [...(by.get(f) ?? []), shortName(s.name)]);
  return [...by.entries()].map(([khz, names]) => ({ khz, names })).sort((a, b) => a.khz - b.khz);
}

/** Where the bursts sit in USB audio tuned to `dial` with the passband
 * `lo`-`hi`: the listed channel whose bursts fit best, else the usual
 * 1440 Hz (the dial on a channel). Returns [audio Hz, the channel's RF]. */
export function hfdlCentre(stations: HfdlStation[], dial: number, lo: number, hi: number): [number, number] {
  let best: [number, number] | null = null;
  for (const c of hfdlChannels(stations)) {
    const a = c.khz * 1000 + HFDL_CENTRE - dial;
    // the burst is ±1.2 kHz wide
    if (a - 1200 < lo - 300 || a + 1200 > hi + 300) continue;
    if (!best || Math.abs(a - (lo + hi) / 2) < Math.abs(best[0] - (lo + hi) / 2)) best = [a, c.khz * 1000];
  }
  return best ?? [HFDL_CENTRE, dial];
}

/** Who an item is about: flight, registration and ICAO address as known. */
export function who(it: HfdlItem): string {
  return [it.flight, it.reg, it.icao].filter((x) => x).join(" ");
}
