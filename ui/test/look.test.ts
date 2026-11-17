// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { colormap, cssColor, isPalette, PALETTE_NAMES } from "../src/view/colormap.ts";
import { pickInterval, timeTicks, utcLabel } from "../src/view/wfticks.ts";

test("every palette runs from its first stop to its last, opaque", () => {
  assert.deepEqual(PALETTE_NAMES, ["web888", "viridis", "heat", "phosphor", "grey"]);
  for (const p of PALETTE_NAMES) {
    const m = colormap(p);
    assert.equal(m.length, 1024);
    for (let i = 3; i < m.length; i += 4) assert.equal(m[i], 255);
  }
  assert.equal(cssColor(0, "grey"), "rgb(0,0,0)");
  assert.equal(cssColor(1, "grey"), "rgb(255,255,255)");
  assert.equal(cssColor(0.5, "grey"), "rgb(128,128,128)");
  assert.equal(cssColor(1, "viridis"), "rgb(253,231,37)");
  // the default is the old palette
  assert.equal(cssColor(0), "rgb(8,10,18)");
  assert.ok(isPalette("heat") && !isPalette("rainbow") && !isPalette(null));
});

/** Lines newest first, `perS` a second, the newest at `t0` (ms). */
const lines = (n: number, perS: number, t0: number) => Float64Array.from({ length: n }, (_, i) => t0 - (i * 1000) / perS);

test("marks at round times, far enough apart", () => {
  // 20 lines/s, marks at least 40 rows apart: every 2 s (40 rows)
  const t0 = Date.UTC(2026, 10, 17, 9, 12, 33, 20);
  const ticks = timeTicks(lines(400, 20, t0), 40);
  assert.ok(ticks.length >= 9, `${ticks.length}`);
  for (const t of ticks) assert.equal(t.t % 2000, 0);
  // the first: 09:12:32, crossed 21 lines down (33.020 -> 32.000 s)
  assert.equal(ticks[0].label, "09:12:32");
  assert.equal(ticks[0].row, 20);
  for (let i = 1; i < ticks.length; i++) assert.equal(ticks[i].row - ticks[i - 1].row, 40);
  // slow lines (1 a second): marks a minute apart, as HH:MM
  const slow = timeTicks(lines(400, 1, t0), 40);
  assert.ok(slow.length >= 6);
  for (const t of slow) assert.match(t.label, /^\d\d:\d\d$/);
});

test("no lines, one line, or a gap: no harm", () => {
  assert.deepEqual(timeTicks(new Float64Array(10).fill(NaN), 40), []);
  assert.deepEqual(timeTicks(Float64Array.of(1e12), 40), []);
  const l = lines(200, 10, Date.UTC(2026, 0, 1, 0, 0, 9, 500));
  l.fill(NaN, 50, 60);
  for (const t of timeTicks(l, 30)) assert.ok(Number.isFinite(t.t));
  assert.equal(utcLabel(Date.UTC(2026, 0, 1, 23, 5, 7), 10), "23:05:07");
  assert.equal(utcLabel(Date.UTC(2026, 0, 1, 23, 5, 0), 300), "23:05");
});

test("the marks' interval holds while the line rate wobbles near a step", () => {
  // 8 lines/s and marks 40 rows apart: 5 s is just enough (40 rows); at
  // 7.9 lines/s it isn't (39.5), so on its own the interval would flip
  // between 5 and 10 s from frame to frame
  assert.equal(pickInterval(8.0, 40), 5);
  assert.equal(pickInterval(7.9, 40), 10);
  assert.equal(pickInterval(7.9, 40, 5), 5);
  assert.equal(pickInterval(8.1, 40, 10), 10);
  // until the rate has really changed
  assert.equal(pickInterval(5, 40, 5), 10);
  assert.equal(pickInterval(40, 40, 10), 1);
  const t0 = Date.UTC(2026, 10, 17, 9, 12, 33, 20);
  let iv = 0;
  for (const perS of [8.0, 7.9, 8.05, 7.95, 8.0]) {
    const ticks = timeTicks(lines(400, perS, t0), 40, iv);
    iv = ticks[0].iv;
    assert.equal(iv, 5, `${perS}`);
  }
});

test("a mark keeps its time as the lines scroll", () => {
  const t0 = Date.UTC(2026, 10, 17, 9, 12, 33, 20);
  const before = timeTicks(lines(400, 10, t0), 40);
  // one more line on top: every mark one row down, the same times
  const after = timeTicks(lines(401, 10, t0 + 100), 40, before[0].iv).filter((t) => t.row > 0);
  const at = new Map(after.map((t) => [t.t, t.row]));
  for (const t of before.slice(0, -1)) assert.equal(at.get(t.t), t.row + 1, t.label);
});
