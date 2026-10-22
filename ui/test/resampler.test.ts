// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { Resampler } from "../src/audio/resampler.ts";

const IN = 12000;
const OUT = 48000;

function tone(n: number, f: number, k0: number): Float32Array {
  const x = new Float32Array(n);
  for (let k = 0; k < n; k++) x[k] = Math.sin((2 * Math.PI * f * (k0 + k)) / IN);
  return x;
}

test("buffers to the target before playing", () => {
  const r = new Resampler(IN, OUT, 150);
  const out = new Float32Array(128);
  r.push(tone(240, 1000, 0)); // 20 ms
  assert.equal(r.pull(out), false);
  assert.ok(out.every((v) => v === 0));
  for (let k = 1; k < 8; k++) r.push(tone(240, 1000, 240 * k)); // 160 ms
  assert.equal(r.pull(out), true);
});

test("plays a tone at the right pitch and keeps the fill near the target", () => {
  const r = new Resampler(IN, OUT, 150);
  const out = new Float32Array(128);
  let k0 = 0;
  const played: number[] = [];
  // server sends 20 ms frames as the card consumes 128-sample quanta,
  // the card running 0.2 % fast against the server clock
  let t = 0;
  for (let q = 0; q < 48000 * 20 / 128; q++) {
    t += 128 / (OUT * 1.002);
    while (k0 / IN < t + 0.16) {
      r.push(tone(240, 1000, k0));
      k0 += 240;
    }
    r.pull(out);
    if (q > 48000 * 18 / 128) played.push(...out);
  }
  assert.equal(r.underruns, 0);
  const fill = r.stats().fillMs;
  assert.ok(Math.abs(fill - 150) < 40, `fill ${fill}`);
  // zero crossings over the last 2 s: 1 kHz tone, 2 per cycle
  let zc = 0;
  for (let k = 1; k < played.length; k++) if (played[k - 1] < 0 && played[k] >= 0) zc++;
  const f = zc / (played.length / OUT);
  assert.ok(Math.abs(f - 1000) < 10, `pitch ${f}`);
});

test("underrun plays silence and rebuffers", () => {
  const r = new Resampler(IN, OUT, 50);
  const out = new Float32Array(128);
  r.push(tone(1200, 500, 0)); // 100 ms
  let n = 0;
  while (r.pull(out)) n++;
  assert.ok(n > 30);
  assert.equal(r.underruns, 1);
  assert.equal(r.pull(out), false);
});
