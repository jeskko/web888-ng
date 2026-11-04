// SPDX-License-Identifier: GPL-3.0-or-later
// The waterfall's colour range (dB): what is usable, so a bad value
// (not a number, floor above the top, nothing between them) can't blank
// the spectrum and waterfall (pure, unit-tested).

/** Least dB between floor and top. */
export const MIN_SPAN = 10;

export interface Range {
  min: number;
  max: number;
}

export const DEFAULT_RANGE: Range = { min: -130, max: -60 };

export function usable(r: Range): boolean {
  return Number.isFinite(r.min) && Number.isFinite(r.max) && r.max - r.min >= MIN_SPAN;
}

/** The range to draw with: `want` if usable, else the first usable of
 * `fallbacks`, else the default. */
export function pick(want: Range, ...fallbacks: Range[]): Range {
  return [want, ...fallbacks].find(usable) ?? DEFAULT_RANGE;
}

/** Floor and top after the user moved the floor to `v`: the top goes along. */
export function setFloor(r: Range, v: number): Range {
  return { min: v, max: Math.max(r.max, v + MIN_SPAN) };
}

/** ... and after moving the top: the floor goes along. */
export function setTop(r: Range, v: number): Range {
  return { min: Math.min(r.min, v - MIN_SPAN), max: v };
}
