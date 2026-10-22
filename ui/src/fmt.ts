// SPDX-License-Identifier: GPL-3.0-or-later
// Frequency formatting, entry and stepping (pure, unit-tested).

import type { Mode } from "./proto.ts";

/** "7 074.000" style: kHz with grouped thousands and 3 decimals (Hz). */
export function formatKhz(hz: number): string {
  const r = Math.round(hz);
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
  return mode === "am" ? 1000 : 10;
}

/** Arrow-key tuning step per mode. */
export function stepFor(mode: Mode): number {
  return mode === "am" ? 1000 : mode === "cw" ? 10 : 100;
}

export function snap(hz: number, step: number): number {
  return Math.round(hz / step) * step;
}

export function clamp(x: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, x));
}

/** Passband presets (Hz, lo..hi relative to the dial) per mode. */
export function bandwidths(mode: Mode): { label: string; lo: number; hi: number }[] {
  switch (mode) {
    case "usb":
      return [1.8, 2.4, 2.7, 3.0].map((k) => ({ label: `${k}k`, lo: 300, hi: 300 + k * 1000 }));
    case "lsb":
      return [1.8, 2.4, 2.7, 3.0].map((k) => ({ label: `${k}k`, lo: -300 - k * 1000, hi: -300 }));
    case "cw":
      return [200, 500, 1000].map((w) => ({ label: `${w}`, lo: -w / 2, hi: w / 2 }));
    case "am":
      return [6, 8, 10].map((k) => ({ label: `${k}k`, lo: (-k * 1000) / 2, hi: (k * 1000) / 2 }));
  }
}
