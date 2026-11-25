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

/** The automatic colour range: floor - 8 dB to floor + 50 dB, the floor
 * following the waterfall's noise floor (one estimate a line) but
 * damped, so the colours don't flicker:
 *
 * * the median of the last `WINDOW_MS` of estimates (one busy or empty
 *   line, a pulse, the quantisation don't move it);
 * * smoothed with a slow time constant (`SLOW_MS`); when the whole
 *   last `FAST_MS` is over `FAR_DB` away on one side (a new band, the
 *   antenna back), it starts again from there and the colours follow
 *   at once;
 * * the colours hold until the smoothed floor is `HOLD_DB` away, then
 *   glide to it (`GLIDE_MS`) and hold again;
 * * a zoom, or a jump of more than half the view, starts afresh. */
export class AutoRange {
  static readonly WINDOW_MS = 2000;
  static readonly SLOW_MS = 4000;
  static readonly FAST_MS = 500;
  static readonly FAR_DB = 10;
  static readonly HOLD_DB = 3;
  static readonly GLIDE_MS = 700;
  static readonly BELOW = 8;
  static readonly ABOVE = 50;

  private win: { t: number; v: number }[] = [];
  private est = NaN;
  private last = NaN;
  private moving = false;
  private fast = false;
  private view: { f0: number; df: number } | null = null;
  /** The floor the colours use (NaN before the first estimate). */
  shown = NaN;

  get range(): Range {
    return { min: this.shown - AutoRange.BELOW, max: this.shown + AutoRange.ABOVE };
  }

  /** One line's noise floor estimate at `t` (ms), from a view whose
   * first bin is at `f0` and bins `df` wide (n bins). */
  push(floor: number, t: number, view?: { f0: number; df: number; n: number }): void {
    if (!Number.isFinite(floor) || !Number.isFinite(t)) return;
    if (view) {
      const v = this.view;
      if (v && (v.df !== view.df || Math.abs(v.f0 - view.f0) > (view.df * view.n) / 2)) this.reset();
      this.view = { f0: view.f0, df: view.df };
    }
    this.win.push({ t, v: floor });
    while (this.win.length > 1 && this.win[0].t < t - AutoRange.WINDOW_MS) this.win.shift();
    let target = median(this.win.map((w) => w.v));
    if (Number.isNaN(this.est)) {
      this.est = this.shown = target;
      this.last = t;
      return;
    }
    const dt = Math.min(1000, Math.max(0, t - this.last));
    this.last = t;
    const k = (tau: number) => 1 - Math.exp(-dt / tau);
    // the last half second far off, all of it: a new floor, follow it now
    const recent = this.win.filter((w) => w.t >= t - AutoRange.FAST_MS).map((w) => w.v);
    const short = median(recent);
    const far = recent.length >= 3 && Math.abs(short - this.est) > AutoRange.FAR_DB && recent.every((v) => Math.sign(v - this.est) === Math.sign(short - this.est));
    if (far) {
      // forget the old floor's estimates; the colours go there quickly
      this.win = this.win.filter((w) => w.t >= t - AutoRange.FAST_MS);
      this.est = target = short;
      this.fast = true;
    }
    this.est += k(AutoRange.SLOW_MS) * (target - this.est);
    if (Math.abs(this.est - this.shown) > AutoRange.HOLD_DB) this.moving = true;
    if (this.moving) {
      this.shown += k(this.fast ? AutoRange.FAST_MS / 2 : AutoRange.GLIDE_MS) * (this.est - this.shown);
      if (Math.abs(this.est - this.shown) < 0.3) this.moving = this.fast = false;
    }
  }

  reset(): void {
    this.win = [];
    this.est = this.shown = this.last = NaN;
    this.moving = this.fast = false;
  }
}

function median(v: number[]): number {
  const s = [...v].sort((a, b) => a - b);
  const m = s.length >> 1;
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
}
