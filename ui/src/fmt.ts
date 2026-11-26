// SPDX-License-Identifier: GPL-3.0-or-later
// Frequency formatting, entry and stepping (pure, unit-tested).

import { t } from "./i18n.ts";
import { isAm, type Mode } from "./proto.ts";

/** "7 074.000" style: kHz with grouped thousands and 3 decimals (Hz). */
/** A converter's offset (Hz) added to every frequency shown (the
 * receiver's `[radio] freq_offset_hz`; 0: none). */
let shown = 0;
export function setShownOffset(hz: number): void {
  shown = Number.isFinite(hz) ? hz : 0;
}
export function shownOffset(): number {
  return shown;
}

/** `hz` (the receiver's) as shown, in kHz with a point and spaced
 * thousands; `raw`: `hz` is already as shown. */
export function formatKhz(hz: number, raw = false): string {
  const r = Math.round(raw ? hz : hz + shown);
  const khz = Math.floor(r / 1000);
  const frac = String(r - khz * 1000).padStart(3, "0");
  const g = String(khz).replace(/\B(?=(\d{3})+(?!\d))/g, " ");
  return `${g}.${frac}`;
}

/** Short label for a scale tick at `step` spacing (Hz). */
export function tickLabel(hz: number, step: number): string {
  if (step >= 1e6) return `${(hz / 1e6).toFixed(0)}M`;
  if (step >= 1e5) return `${(hz / 1e6).toFixed(1)}M`;
  if (step >= 1e4) return `${(hz / 1e6).toFixed(2)}M`;
  if (step >= 1e3) return `${(hz / 1e3).toFixed(0)}k`;
  if (step >= 100) return `${(hz / 1e3).toFixed(1)}k`;
  return `${(hz / 1e3).toFixed(2)}k`;
}

/** Tick spacing (1-2-5 series) giving about `want` ticks over `span`. */
export function tickStep(span: number, want: number): number {
  const raw = span / Math.max(1, want);
  const p = 10 ** Math.floor(Math.log10(raw));
  for (const m of [1, 2, 5, 10]) if (m * p >= raw) return m * p;
  return 10 * p;
}

/**
 * Parse typed frequency entry. Bare numbers are kHz ("7074", "7074.5");
 * suffixes k/M/Hz select the unit ("7.074M", "7074000hz").
 */
export function parseFreq(s: string): number | null {
  const m = s.trim().toLowerCase().replace(/\s+/g, "").match(/^([0-9]*\.?[0-9]+)(k|khz|m|mhz|hz)?$/);
  if (!m) return null;
  const x = parseFloat(m[1]);
  const unit = m[2] ?? "k";
  const mult = unit.startsWith("m") ? 1e6 : unit === "hz" ? 1 : 1e3;
  return x * mult;
}

/** Click-tune snap per mode. */
export function snapFor(mode: Mode): number {
  return mode === "nbfm" ? 6250 : isAm(mode) ? 1000 : 10;
}

/** Arrow-key tuning step per mode. */
export function stepFor(mode: Mode): number {
  // FM channels: 12.5 kHz apart (and 25, 6.25)
  return mode === "nbfm" ? 12500 : isAm(mode) ? 1000 : mode === "cw" ? 10 : 100;
}

export function snap(hz: number, step: number): number {
  return Math.round(hz / step) * step;
}

/** Tuning step choices (Hz); 0: the mode's own (`stepFor`, `snapFor`). */
export const STEPS = [0, 1, 10, 50, 100, 500, 1000, 5000, 9000, 10000];

export function stepLabel(step: number): string {
  return step === 0 ? t("auto") : step >= 1000 ? `${step / 1000}k` : `${step}`;
}

/** The next multiple of `step` from `hz` in direction `dir` (+1 / -1):
 * off the grid, the nearest one that way; on it, one step. */
export function stepTo(hz: number, step: number, dir: number): number {
  const k = hz / step;
  const r = Math.round(k);
  const on = Math.abs(k - r) < 1e-6;
  return (on ? r + dir : dir > 0 ? Math.ceil(k) : Math.floor(k)) * step;
}

export function clamp(x: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, x));
}

/** Twin passband tuning's reach: each edge moves this far (Hz). */
export const PBT_MAX = 2000;

/** The passband with twin PBT applied: `base` (a preset, or a passband a
 * decoder set) with its low edge moved by `off.lo` and its high edge by
 * `off.hi`, kept as the server keeps it (edges within 5 kHz at 12 kS/s,
 * at least 50 Hz wide; the low edge wins when they meet). */
export function pbtEdges(base: { lo: number; hi: number }, off: { lo: number; hi: number }, rate = 12000): [number, number] {
  const e = (5000 * rate) / 12000;
  const lo = clamp(base.lo + off.lo, -e, e - 50);
  const hi = clamp(base.hi + off.hi, lo + 50, e);
  return [lo, hi];
}

/** Passband presets (Hz, lo..hi relative to the dial) per mode. */
export function bandwidths(mode: Mode, rate = 12000): { label: string; lo: number; hi: number }[] {
  const wide = rate >= 24000;
  switch (mode) {
    case "usb":
      return [1.8, 2.4, 2.7, 3.0].map((k) => ({ label: `${k}k`, lo: 300, hi: 300 + k * 1000 }));
    case "lsb":
      return [1.8, 2.4, 2.7, 3.0].map((k) => ({ label: `${k}k`, lo: -300 - k * 1000, hi: -300 }));
    case "cw":
      return [200, 500, 1000].map((w) => ({ label: `${w}`, lo: -w / 2, hi: w / 2 }));
    case "am":
    case "sam":
    case "sas":
    case "iq":
    case "nbfm":
      return (wide ? [6, 10, 16, 20] : [6, 8, 10]).map((k) => ({ label: `${k}k`, lo: (-k * 1000) / 2, hi: (k * 1000) / 2 }));
    // one sideband and the carrier (50 Hz inside the edge)
    case "sal":
      return (wide ? [4, 5, 8, 10] : [3, 4, 5]).map((k) => ({ label: `${k}k`, lo: -k * 1000, hi: 50 }));
    case "sar":
      return (wide ? [4, 5, 8, 10] : [3, 4, 5]).map((k) => ({ label: `${k}k`, lo: -50, hi: k * 1000 }));
  }
}
