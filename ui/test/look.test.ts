// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { colormap, cssColor, isPalette, PALETTE_NAMES } from "../src/view/colormap.ts";
import { timeTicks, utcLabel } from "../src/view/wfticks.ts";

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
