// SPDX-License-Identifier: GPL-3.0-or-later
// The HF propagation forecast (`/api/propagation`): its shape, and the
// words and colours the panel shows it with (pure, unit-tested).

import { bandOf } from "./bands.ts";
import { N_, t } from "./i18n.ts";
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

const MONTHS = [
  N_("January"),
  N_("February"),
  N_("March"),
  N_("April"),
  N_("May"),
  N_("June"),
  N_("July"),
  N_("August"),
  N_("September"),
  N_("October"),
  N_("November"),
  N_("December"),
];

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
  const month = t(MONTHS[f.month - 1]);
  const when = f.day !== null ? t("{day} {month} {year}", { day: f.day, month, year: f.year }) : t("{month} {year}", { month, year: f.year });
  const src = f.ssn_source.kind === "config" ? t("set by the owner") : f.ssn_source.kind === "noaa" ? t("NOAA's prediction") : t("built-in table");
  // i18n: SSN = sunspot number, the solar activity the forecast assumes
  return t("{when}, sunspot number {ssn} ({src})", { when, ssn: f.ssn, src });
}

/** How it was made, in a sentence. */
export function forecastBasis(f: Forecast): string {
  const model = f.engine === "truecast" ? t("VOACAP adjusted to the day (hfcast Truecast)") : t("VOACAP, the month's median");
  const v = { model, power: f.power_w };
  // i18n: the receiving site's noise level (VOACAP's man-made noise at 3 MHz): remote, rural, residential, noisy
  const dbw = f.noise_dbw;
  if (dbw <= -153) return t("{model}; {power} W with simple (isotropic) antennas at both ends, a remote receiver.", v);
  if (dbw <= -150) return t("{model}; {power} W with simple (isotropic) antennas at both ends, a rural receiver.", v);
  if (dbw <= -145) return t("{model}; {power} W with simple (isotropic) antennas at both ends, a residential receiver.", v);
  return t("{model}; {power} W with simple (isotropic) antennas at both ends, a noisy (industrial) receiver.", v);
}

/** A warning when the field is disturbed: VOACAP knows only quiet days. */
export function stormNote(kp: PropReply["kp"]): string | null {
  if (!kp || kp.max24 < 5) return null;
  // i18n: Kp = planetary geomagnetic index (0-9)
  return t("Geomagnetic storm: Kp reached {kp} in the last 24 hours. The forecast is too optimistic, especially on long and polar paths.", { kp: kp.max24.toFixed(1) });
}

/** A cell's tooltip. */
export function cellTitle(f: Forecast, p: PropPath, mode: number, band: number, hour: number): string {
  const r = p.rel[mode]?.[band]?.[hour];
  const s = p.snr[band]?.[hour];
  const hh = String(hour).padStart(2, "0");
  const mode_ = f.modes[mode]?.name ?? "";
  const bandN = bandName(f.bands[band]);
  if (r === null || r === undefined) {
    // i18n: reliability = VOACAP circuit reliability, % of days the path works; here the forecast has no figure for that hour
    return s === null || s === undefined
      ? t("{path}, {band}, {hour} UTC: no reliability figure for {mode}", { path: p.name, band: bandN, hour: hh, mode: mode_ })
      : t("{path}, {band}, {hour} UTC: no reliability figure for {mode}; SNR {snr} dB in 1 Hz", { path: p.name, band: bandN, hour: hh, mode: mode_, snr: Math.round(s) });
  }
  const pct = Math.round(r * 100);
  // i18n: SNR = signal-to-noise ratio, in a 1 Hz bandwidth
  return s === null || s === undefined
    ? t("{path}, {band}, {hour} UTC: {mode} gets through on {pct} % of days", { path: p.name, band: bandN, hour: hh, mode: mode_, pct })
    : t("{path}, {band}, {hour} UTC: {mode} gets through on {pct} % of days, SNR {snr} dB in 1 Hz", { path: p.name, band: bandN, hour: hh, mode: mode_, pct, snr: Math.round(s) });
}
