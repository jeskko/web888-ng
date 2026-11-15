// SPDX-License-Identifier: GPL-3.0-or-later
// Wheel input to whole steps (zoom ×2, or a tuning step). A mouse wheel
// sends one event per notch (100 px in Chromium, 3 lines in Firefox); a
// trackpad sends dozens of small pixel deltas per gesture, momentum
// included, so one step per event zooms a trackpad far too fast. Deltas
// are summed instead, a step per WHEEL_PX (one notch); a trackpad pinch
// (ctrl+wheel, Chromium scales by exp(-deltaY/100) per event, so ×2 is
// ~69) a step per PINCH_PX. At most one step per event, and a pause or a
// change of direction starts afresh.

/** Pixels of wheel travel per step. */
export const WHEEL_PX = 100;
/** Pixels of pinch (ctrl+wheel) per step. */
export const PINCH_PX = 70;
/** A gesture's events come at least this often (ms). */
export const PAUSE_MS = 250;

/** A wheel delta in pixels (deltaMode 1: lines, 2: pages). */
export function wheelPx(delta: number, mode: number): number {
  return mode === 1 ? delta * 40 : mode === 2 ? delta * 800 : delta;
}

export class WheelSteps {
  private acc = 0;
  private last = -Infinity;

  /** Steps for this event's delta (pixels; positive = wheel down/back):
   * −1, 0 or +1, the sign of the travel. */
  push(px: number, pinch: boolean, ms: number): number {
    if (ms - this.last > PAUSE_MS || Math.sign(px) * Math.sign(this.acc) < 0) this.acc = 0;
    this.last = ms;
    this.acc += px / (pinch ? PINCH_PX : WHEEL_PX);
    if (Math.abs(this.acc) < 1) return 0;
    const n = Math.sign(this.acc);
    // one step per event; what's left over counts towards the next
    this.acc = Math.max(-0.99, Math.min(0.99, this.acc - n));
    return n;
  }
}
