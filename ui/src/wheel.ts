// SPDX-License-Identifier: GPL-3.0-or-later
// The phone's VFO wheel: rotation to dial steps, and the step choices
// (pure, unit-tested).

import { stepTo } from "./fmt.ts";

/** Wheel resolutions (Hz): the centre of the wheel steps through them. */
export const WHEEL_STEPS = [100, 500, 1000, 5000, 25000, 50000, 100000];

/** Clicks in one turn of the wheel. */
export const CLICKS_PER_TURN = 24;

export function wheelLabel(step: number): string {
  return `${step / 1000} kHz`;
}

/** The next resolution after `step` (wraps); an unknown one starts over. */
export function nextWheelStep(step: number): number {
  const i = WHEEL_STEPS.indexOf(step);
  return WHEEL_STEPS[(i + 1) % WHEEL_STEPS.length];
}

/** Angle change a -> b (radians), the short way round, in (-PI, PI]. */
export function angleDelta(a: number, b: number): number {
  let d = b - a;
  while (d > Math.PI) d -= 2 * Math.PI;
  while (d <= -Math.PI) d += 2 * Math.PI;
  return d;
}

/**
 * Collects rotation into whole clicks: returns the clicks to make now
 * (positive: clockwise) and the rotation left over (radians).
 */
export function takeClicks(rest: number, delta: number, perTurn = CLICKS_PER_TURN): { clicks: number; rest: number } {
  const per = (2 * Math.PI) / perTurn;
  const x = rest + delta;
  const clicks = Math.trunc(x / per);
  return { clicks, rest: x - clicks * per };
}

/**
 * Where `clicks` clicks of `step` take the dial: the first click lands
 * on the step's grid (as the arrow buttons do), the others follow.
 */
export function wheelTarget(hz: number, step: number, clicks: number): number {
  if (clicks === 0) return hz;
  const dir = clicks > 0 ? 1 : -1;
  return stepTo(hz, step, dir) + (Math.abs(clicks) - 1) * step * dir;
}
