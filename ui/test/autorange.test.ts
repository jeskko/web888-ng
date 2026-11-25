// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { AutoRange } from "../src/view/range.ts";

/** A repeatable noisy floor: ±1.5 dB jitter (the 20th percentile of one
 * line, quantised), and now and then a line 8 dB off (a burst, a busy
 * line). */
function floors(n: number, base: (i: number) => number, seed = 1) {
  let s = seed;
  const rnd = () => ((s = (s * 1103515245 + 12345) & 0x7fffffff) / 0x7fffffff);
  return Array.from({ length: n }, (_, i) => base(i) + (rnd() - 0.5) * 3 + (rnd() < 0.05 ? (rnd() < 0.5 ? -8 : 8) : 0));
}

/** The old tracker: an EMA of 0.1 a line. */
function oldTrack(f: number[]) {
  let m = NaN;
  return f.map((v) => (m = Number.isNaN(m) ? v : m + 0.1 * (v - m)));
}

const LINE_MS = 50;
const steps = (s: number[]) => s.slice(1).reduce((a, v, i) => a + Math.abs(v - s[i]), 0);

test("the auto range holds still on a steady floor with jitter and outliers", () => {
  const f = floors(1200, () => -120);
  const a = new AutoRange();
  const shown = f.map((v, i) => (a.push(v, i * LINE_MS), a.shown));
  const old = oldTrack(f);
  // after the start, the colours barely move; before, they wandered all the time
  const tail = shown.slice(100);
  assert.ok(Math.max(...tail) - Math.min(...tail) < 1, `new range moved ${Math.max(...tail) - Math.min(...tail)} dB`);
  assert.ok(steps(tail) < 1, `new total movement ${steps(tail)}`);
  assert.ok(steps(old.slice(100)) > 50 * steps(tail) + 20, `old total movement ${steps(old.slice(100))}`);
  assert.ok(Math.abs(tail.at(-1)! + 120) < 1);
});

test("it follows a real change of the floor within a few seconds", () => {
  // the band opens: the floor 6 dB up after 30 s
  const f = floors(1600, (i) => (i < 600 ? -120 : -114), 7);
  const a = new AutoRange();
  const shown = f.map((v, i) => (a.push(v, i * LINE_MS), a.shown));
  const t = shown.findIndex((v, i) => i > 600 && v > -116);
  assert.ok(t > 600 && (t - 600) * LINE_MS < 8000, `reached -116 after ${((t - 600) * LINE_MS) / 1000} s`);
  assert.ok(Math.abs(shown.at(-1)! + 114) < 1.5, `ends at ${shown.at(-1)}`);
});

test("a big jump (another band) is followed fast, a zoom starts afresh", () => {
  const a = new AutoRange();
  const view = { f0: 7e6, df: 100, n: 1000 };
  for (let i = 0; i < 200; i++) a.push(-120, i * LINE_MS, view);
  for (let i = 200; i < 260; i++) a.push(-95, i * LINE_MS, view);
  assert.ok(a.shown > -100, `3 s after a 25 dB jump: ${a.shown}`);
  a.push(-80, 260 * LINE_MS, { ...view, df: 50 });
  assert.equal(a.shown, -80);
  assert.deepEqual(a.range, { min: -88, max: -30 });
});

test("bad estimates are ignored", () => {
  const a = new AutoRange();
  a.push(NaN, 0);
  assert.ok(Number.isNaN(a.shown));
  a.push(-110, 50);
  a.push(Infinity, 100);
  assert.equal(a.shown, -110);
});
