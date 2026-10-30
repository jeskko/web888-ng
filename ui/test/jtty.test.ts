// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { JTTY_DIALS, JttyFeed, toRows } from "../src/jtty.ts";

test("12 kHz mono passes through as a copy", () => {
  const f = new JttyFeed();
  const pcm = Float32Array.from([1, 2, 3]);
  const out = f.push(pcm, 12_000, false);
  assert.deepEqual([...out], [1, 2, 3]);
  assert.notEqual(out.buffer, pcm.buffer, "a copy: the player takes the frame's buffer");
});

test("stereo frames are averaged", () => {
  const out = new JttyFeed().push(Float32Array.from([1, 3, -1, 1]), 12_000, true);
  assert.deepEqual([...out], [2, 0]);
});

test("other rates are resampled to 12 kHz, the phase kept across frames", () => {
  const f = new JttyFeed();
  // a 24 kHz ramp in frames of 7: every other sample, without gaps or repeats
  const got: number[] = [];
  for (let k = 0; k < 10; k++) {
    const frame = Float32Array.from({ length: 7 }, (_, i) => 7 * k + i);
    got.push(...f.push(frame, 24_000, false));
  }
  const want = Array.from({ length: got.length }, (_, i) => 2 * i);
  assert.deepEqual(got, want);
  assert.ok(got.length >= 34 && got.length <= 35);
});

test("rows carry the start time and the RF frequency", () => {
  const rows = toRows([{ text: "CQ DE K1ABC", hz: 1500, start: 12_000, frames: 3 }], 14_090_000, 36_000, 1000);
  assert.deepEqual(rows, [{ t: 998, rf: 14_091_500, hz: 1500, text: "CQ DE K1ABC", frames: 3 }]);
});

test("the dial list is in band order", () => {
  for (let k = 1; k < JTTY_DIALS.length; k++) assert.ok(JTTY_DIALS[k].hz > JTTY_DIALS[k - 1].hz);
  assert.ok(JTTY_DIALS.some((d) => d.hz === 14_090_000));
});
