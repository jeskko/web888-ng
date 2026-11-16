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
  // a 36 kHz ramp in frames of 7: every third sample, without gaps or repeats
  const got: number[] = [];
  for (let k = 0; k < 10; k++) {
    const frame = Float32Array.from({ length: 7 }, (_, i) => 7 * k + i);
    got.push(...f.push(frame, 36_000, false));
  }
  const want = Array.from({ length: got.length }, (_, i) => 3 * i);
  assert.deepEqual(got, want);
  assert.ok(got.length >= 23 && got.length <= 24);
});

/** Amplitude of the tone at `hz` in `x` at 12 kS/s (Hann-windowed DFT). */
function amp12(x: number[], hz: number): number {
  let re = 0;
  let im = 0;
  let ws = 0;
  for (let k = 0; k < x.length; k++) {
    const w = 0.5 - 0.5 * Math.cos((2 * Math.PI * k) / (x.length - 1));
    const a = (2 * Math.PI * hz * k) / 12_000;
    re += x[k] * w * Math.cos(a);
    im -= x[k] * w * Math.sin(a);
    ws += w;
  }
  return (2 * Math.hypot(re, im)) / ws;
}

test("24 kS/s is low-passed and halved: no aliases, the same across frames", () => {
  const tone = (hz: number, n: number) => Float32Array.from({ length: n }, (_, k) => Math.sin((2 * Math.PI * hz * k) / 24_000));
  const run = (x: Float32Array, size: number) => {
    const f = new JttyFeed();
    const out: number[] = [];
    for (let i = 0; i < x.length; i += size) out.push(...f.push(x.subarray(i, i + size), 24_000, false));
    return out;
  };
  const n = 24_000;
  // 1 kHz passes (after the filter's start)
  const a = run(tone(1000, n), 512).slice(100);
  assert.ok(Math.abs(amp12(a, 1000) - 1) < 0.01, `${amp12(a, 1000)}`);
  assert.equal(run(tone(1000, n), 512).length, n / 2);
  // 9 kHz would fold to 3 kHz: 70 dB down
  const b = run(tone(9000, n), 512).slice(100);
  assert.ok(amp12(b, 3000) < 3e-4, `${amp12(b, 3000)}`);
  // frames of any size give the same output
  const whole = run(tone(1234, 4001), 4001);
  const bits = run(tone(1234, 4001), 7);
  assert.equal(bits.length, whole.length);
  for (let k = 0; k < whole.length; k++) assert.ok(Math.abs(bits[k] - whole[k]) < 1e-6);
});

test("rows carry the start time and the RF frequency", () => {
  const rows = toRows([{ text: "CQ DE K1ABC", hz: 1500, start: 12_000, frames: 3 }], 14_090_000, 36_000, 1000);
  assert.deepEqual(rows, [{ t: 998, rf: 14_091_500, hz: 1500, text: "CQ DE K1ABC", frames: 3 }]);
});

test("the dial list is in band order", () => {
  for (let k = 1; k < JTTY_DIALS.length; k++) assert.ok(JTTY_DIALS[k].hz > JTTY_DIALS[k - 1].hz);
  assert.ok(JTTY_DIALS.some((d) => d.hz === 14_090_000));
});
