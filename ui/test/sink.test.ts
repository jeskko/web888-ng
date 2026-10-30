// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { PcmSink, type SinkStats } from "../src/audio/sink.ts";

const IN = 12000;
const OUT = 48000;

function tone(n: number, k0: number): Float32Array {
  const x = new Float32Array(n);
  for (let k = 0; k < n; k++) x[k] = Math.sin((2 * Math.PI * 1000 * (k0 + k)) / IN);
  return x;
}

// the main-thread fallback: 4096-frame blocks, a 300 ms buffer
test("big blocks: plays once the buffer is full, without underruns", () => {
  const stats: SinkStats[] = [];
  const s = new PcmSink(OUT, 300, (x) => stats.push(x));
  const out = [new Float32Array(4096), new Float32Array(4096)];
  s.render(out);
  assert.ok(out[0].every((v) => v === 0), "silent before the rate is known");
  s.message({ rate: IN });
  let k0 = 0;
  let played = 0;
  // the server's 20 ms frames, the card's 85 ms blocks, for 10 s
  for (let t = 0; t < 10000; t += 20) {
    s.message(tone(240, k0));
    k0 += 240;
    while (played * 1000 < (t - 400) * OUT) {
      s.render(out);
      played += 4096;
    }
  }
  const last = stats[stats.length - 1];
  assert.ok(last.playing);
  assert.equal(last.underruns, 0);
  // at block boundaries: a bit under 5 a second with 85 ms blocks
  assert.ok(stats.length >= 30, `stats about 5 times a second (${stats.length} in 10 s)`);
  assert.ok(out[0].some((v) => Math.abs(v) > 0.5), "the tone comes out");
});

test("stereo frames go to their ears; mono is copied by the filters", () => {
  const s = new PcmSink(OUT, 50, () => {});
  s.message({ rate: IN });
  const left = new Float32Array(1200).fill(0.5);
  const right = new Float32Array(1200).fill(-0.25);
  s.message({ left, right });
  const out = [new Float32Array(1024), new Float32Array(1024), new Float32Array(1024)];
  s.render(out);
  assert.ok(Math.abs(out[0][512] - 0.5) < 1e-6);
  assert.ok(Math.abs(out[1][512] + 0.25) < 1e-6);
  assert.deepEqual(out[2], out[0], "a third channel copies the left");
});
