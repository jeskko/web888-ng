// SPDX-License-Identifier: GPL-3.0-or-later
// Places for the map (pure, unit-tested): Maidenhead locators, great
// circle distance and bearing, the subsolar point (for the greyline), and
// the sender and locator of FT8/FT4 and WSPR messages.

/** Latitude, longitude in degrees. */
export type LatLon = [number, number];

const GRID = /^[A-R]{2}(?:[0-9]{2}(?:[A-X]{2}(?:[0-9]{2})?)?)?$/;

/** The centre of a Maidenhead locator (2, 4, 6 or 8 characters), or null. */
export function gridToLatLon(grid: string): LatLon | null {
  const g = grid.trim().toUpperCase();
  if (!GRID.test(g)) return null;
  const c = (k: number) => g.charCodeAt(k);
  let lon = (c(0) - 65) * 20 - 180;
  let lat = (c(1) - 65) * 10 - 90;
  let w = 20;
  let h = 10;
  if (g.length >= 4) {
    lon += (c(2) - 48) * 2;
    lat += c(3) - 48;
    w = 2;
    h = 1;
  }
  if (g.length >= 6) {
    lon += ((c(4) - 65) * 2) / 24;
    lat += (c(5) - 65) / 24;
    w = 2 / 24;
    h = 1 / 24;
  }
  if (g.length >= 8) {
    lon += ((c(6) - 48) * 2) / 240;
    lat += (c(7) - 48) / 240;
    w = 2 / 240;
    h = 1 / 240;
  }
  return [lat + h / 2, lon + w / 2];
}

/** The locator of a place, `n` characters (2, 4, 6 or 8). */
export function latLonToGrid([lat, lon]: LatLon, n = 6): string {
  let x = Math.min(Math.max(lon + 180, 0), 359.999999);
  let y = Math.min(Math.max(lat + 90, 0), 179.999999);
  const A = (k: number) => String.fromCharCode(65 + k);
  let out = A(Math.floor(x / 20)) + A(Math.floor(y / 10));
  x %= 20;
  y %= 10;
  if (n >= 4) {
    out += String(Math.floor(x / 2)) + String(Math.floor(y));
    x %= 2;
    y %= 1;
  }
  if (n >= 6) {
    out += A(Math.floor(x * 12)).toLowerCase() + A(Math.floor(y * 24)).toLowerCase();
    x %= 1 / 12;
    y %= 1 / 24;
  }
  if (n >= 8) out += String(Math.floor(x * 120)) + String(Math.floor(y * 240));
  return out;
}

const R = 6371;
const rad = Math.PI / 180;

/** Great circle distance, km. */
export function distanceKm(a: LatLon, b: LatLon): number {
  const [p1, p2] = [a[0] * rad, b[0] * rad];
  const dp = p2 - p1;
  const dl = (b[1] - a[1]) * rad;
  const h = Math.sin(dp / 2) ** 2 + Math.cos(p1) * Math.cos(p2) * Math.sin(dl / 2) ** 2;
  return 2 * R * Math.asin(Math.min(1, Math.sqrt(h)));
}

/** Initial great circle bearing from `a` to `b`, degrees 0..360. */
export function bearing(a: LatLon, b: LatLon): number {
  const [p1, p2] = [a[0] * rad, b[0] * rad];
  const dl = (b[1] - a[1]) * rad;
  const y = Math.sin(dl) * Math.cos(p2);
  const x = Math.cos(p1) * Math.sin(p2) - Math.sin(p1) * Math.cos(p2) * Math.cos(dl);
  return (Math.atan2(y, x) / rad + 360) % 360;
}

/** Where the sun is overhead at Unix time `t` (s), to about a tenth of a
 * degree. */
export function subsolar(t: number): LatLon {
  const d = t / 86400 - 10957.5; // days since J2000.0
  const g = (357.529 + 0.98560028 * d) * rad;
  const q = 280.459 + 0.98564736 * d;
  const L = (q + 1.915 * Math.sin(g) + 0.02 * Math.sin(2 * g)) * rad;
  const e = (23.439 - 0.00000036 * d) * rad;
  const ra = Math.atan2(Math.cos(e) * Math.sin(L), Math.cos(L)) / rad;
  const dec = Math.asin(Math.sin(e) * Math.sin(L)) / rad;
  // equation of time, degrees of hour angle
  let eqt = (((q - ra) % 360) + 540) % 360 - 180;
  if (Math.abs(eqt) > 20) eqt = 0;
  const utcDeg = ((t % 86400) / 86400) * 360;
  let lon = 180 - utcDeg + eqt;
  lon = ((((lon + 180) % 360) + 360) % 360) - 180;
  return [dec, lon];
}

const GRID4 = /^[A-R]{2}[0-9]{2}$/;
const GRID46 = /^[A-R]{2}[0-9]{2}(?:[A-X]{2})?$/;

/** Looks like a callsign: letters and digits (a slash part), a digit
 * among them, a letter at the end of the base call. */
export function isCall(s: string): boolean {
  if (s.length < 3 || s.length > 13 || !/^[A-Z0-9/]+$/.test(s)) return false;
  return s.split("/").some((p) => /[0-9]/.test(p) && /[A-Z]/.test(p) && p.length >= 3);
}

/** The sending station of an FT8/FT4 message and its locator when it
 * says: "CQ [DX|POTA|..] CALL [GRID]", "TO CALL [GRID|report|RR73]".
 * Hashed calls (`<...>`) are unknown. */
export function ftxSender(text: string): { call?: string; grid?: string } {
  const t = text.trim().toUpperCase().split(/\s+/);
  const clean = (c: string | undefined) => {
    const s = (c ?? "").replace(/^<|>$/g, "");
    return isCall(s) ? s : undefined;
  };
  const gridAt = (k: number) => (t[k] && GRID4.test(t[k]) && t[k] !== "RR73" ? t[k] : undefined);
  if (t[0] === "CQ" || t[0] === "QRZ") {
    const k = clean(t[1]) ? 1 : 2;
    const call = clean(t[k]);
    return call ? { call, grid: gridAt(k + 1) } : {};
  }
  const call = clean(t[1]);
  return call ? { call, grid: gridAt(2) } : {};
}

/** A WSPR message's sender and locator: "CALL GRID DBM". */
export function wsprSender(text: string): { call?: string; grid?: string } {
  const t = text.trim().toUpperCase().split(/\s+/);
  const call = (t[0] ?? "").replace(/^<|>$/g, "");
  if (!isCall(call)) return {};
  return { call, grid: t[1] && GRID46.test(t[1]) ? t[1] : undefined };
}

/** The last locator heard from each call (a call's later messages
 * mostly don't say it). */
export class GridBook {
  private m = new Map<string, string>();
  private max: number;
  constructor(max = 20000) {
    this.max = max;
  }
  note(call: string | undefined, grid: string | undefined): void {
    if (!call || !grid) return;
    this.m.delete(call);
    this.m.set(call, grid);
    if (this.m.size > this.max) this.m.delete(this.m.keys().next().value as string);
  }
  get(call: string | undefined): string | undefined {
    return call ? this.m.get(call) : undefined;
  }
  get size(): number {
    return this.m.size;
  }
}
