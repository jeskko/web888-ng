// SPDX-License-Identifier: GPL-3.0-or-later
// The HF propagation forecast (`/api/propagation`): its shape, and the
// words and colours the panel shows it with (pure, unit-tested).

import { bandOf } from "./bands.ts";
import { cssColor } from "./view/colormap.ts";

export interface PropMode {
  name: string;
  snr_db: number;
}

export interface PropPath {
  name: string;
  lat: number;
  lon: number;
  km: number;
  bearing: number;
  muf: (number | null)[];
  fot: (number | null)[];
  /** Per mode: the LUF per hour, the reliability per [band][hour]. */
  luf: (number | null)[][];
  rel: (number | null)[][][];
  /** [band][hour], dB in 1 Hz. */
  snr: (number | null)[][];
  error: string | null;
}

export type SsnSource = { kind: "config" } | { kind: "noaa"; fetched: number } | { kind: "builtin" };

export interface Forecast {
  computed: number;
  year: number;
  month: number;
  day: number | null;
  ssn: number;
  ssn_source: SsnSource;
  engine: "voacap" | "truecast";
  power_w: number;
  noise_dbw: number;
  rx: { locator: string; lat: number; lon: number };
  modes: PropMode[];
  bands: number[];
  paths: PropPath[];
}

export interface PropReply {
  enabled: boolean;
  fetch: boolean;
  problem: string | null;
  fetch_errors: string[];
  kp: { time: string; kp: number; max24: number; fetched: number } | null;
  forecast: Forecast | null;
}

const MONTHS = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/** A band's name: "20 m" for an amateur band, else its frequency. */
export function bandName(mhz: number): string {
  const b = bandOf(mhz * 1e6);
  return b ? `${b.name} m` : `${mhz} MHz`;
}

/** Where to tune for a band: the amateur band's usual spot, else the
 *  frequency itself (Hz). */
export function bandHz(mhz: number): number {
  return bandOf(mhz * 1e6)?.hz ?? Math.round(mhz * 1e6);
}

/** A reliability's colour (none: grey). */
export function relColor(r: number | null): string {
  return r === null ? "transparent" : cssColor(r, "viridis");
}

/** What the forecast is: "November 2026, sunspot number 83 (NOAA's
 *  prediction)". */
export function forecastTitle(f: Forecast): string {
  const when = `${f.day !== null ? `${f.day} ` : ""}${MONTHS[f.month - 1]} ${f.year}`;
  const src = f.ssn_source.kind === "config" ? "set by the owner" : f.ssn_source.kind === "noaa" ? "NOAA's prediction" : "built-in table";
  return `${when}, sunspot number ${f.ssn} (${src})`;
}

/** How it was made, in a sentence. */
export function forecastBasis(f: Forecast): string {
  const model = f.engine === "truecast" ? "VOACAP adjusted to the day (hfcast Truecast)" : "VOACAP, the month's median";
  return `${model}; ${f.power_w} W with simple (isotropic) antennas at both ends, a ${noiseName(f.noise_dbw)} receiver.`;
}

function noiseName(dbw: number): string {
  if (dbw <= -153) return "remote";
  if (dbw <= -150) return "rural";
  if (dbw <= -145) return "residential";
  return "noisy (industrial)";
}

/** A warning when the field is disturbed: VOACAP knows only quiet days. */
export function stormNote(kp: PropReply["kp"]): string | null {
  if (!kp || kp.max24 < 5) return null;
  return `Geomagnetic storm: Kp reached ${kp.max24.toFixed(1)} in the last 24 hours. The forecast is too optimistic, especially on long and polar paths.`;
}

/** A cell's tooltip. */
export function cellTitle(f: Forecast, p: PropPath, mode: number, band: number, hour: number): string {
  const r = p.rel[mode]?.[band]?.[hour];
  const s = p.snr[band]?.[hour];
  const hh = String(hour).padStart(2, "0");
  const rel = r === null || r === undefined ? "–" : `${Math.round(r * 100)} %`;
  const snr = s === null || s === undefined ? "" : `, SNR ${Math.round(s)} dB in 1 Hz`;
  return `${p.name}, ${bandName(f.bands[band])}, ${hh} UTC: ${f.modes[mode]?.name ?? ""} gets through on ${rel} of days${snr}`;
}
