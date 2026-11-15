// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { PAUSE_MS, WheelSteps, wheelPx } from "../src/wheelzoom.ts";

test("a mouse notch is one step, whichever unit it comes in", () => {
  const w = new WheelSteps();
  assert.equal(w.push(-100, false, 0), -1);
  assert.equal(w.push(100, false, 1000), 1);
  assert.equal(w.push(wheelPx(3, 1), false, 2000), 1); // Firefox: 3 lines
});

test("a trackpad gesture zooms by its travel, not its event count", () => {
  const w = new WheelSteps();
  // 60 events of 4 px in a second: 240 px, two steps (one per event before)
  let n = 0;
  for (let i = 0; i < 60; i++) n += w.push(-4, false, i * 16);
  assert.equal(n, -2);
});

test("a pinch (ctrl+wheel) has its own scale", () => {
  const w = new WheelSteps();
  let n = 0;
  for (let i = 0; i < 15; i++) n += w.push(-5, true, i * 16); // 75 px: x2 (70 per step)
  assert.equal(n, -1);
});

test("one step per event at most; a pause or a turn starts afresh", () => {
  const w = new WheelSteps();
  assert.equal(w.push(-1000, false, 0), -1);
  assert.equal(w.push(-50, false, 10), -1); // the rest counts towards the next
  assert.equal(w.push(-50, false, 20 + PAUSE_MS), 0); // after a pause: afresh
  assert.equal(w.push(80, false, 30 + PAUSE_MS), 0); // turned: afresh
  assert.equal(w.push(30, false, 40 + PAUSE_MS), 1);
});
