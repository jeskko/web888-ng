// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { MODES, SampleClock, ServerClock, Slotter, budgetMs, toRows } from "../src/digi.ts";

test("the server clock takes the least delayed line, and follows a stepped clock", () => {
  const c = new ServerClock();
  assert.equal(c.known, false);
  assert.equal(c.now(1000), 1000, "the local clock until a line came");
  // the server is 5 s ahead; lines arrive 0.3, 0.1, 0.2 s late
  c.observe(105, 100.3);
  c.observe(106, 101.1);
  c.observe(107, 102.2);
  assert.ok(Math.abs(c.now(200) - 204.9) < 1e-3, `${c.now(200)}`);
  // one line 3 s late changes nothing
  c.observe(108, 106);
  assert.ok(Math.abs(c.now(200) - 204.9) < 1e-3);
  // the local clock stepped 10 s forward: after 20 lines it follows
  for (let k = 0; k < 20; k++) c.observe(110 + k, 115.1 + k);
  assert.ok(Math.abs(c.now(200) - 194.9) < 1e-3, `${c.now(200)}`);
});

test("the sample clock takes the least delayed block", () => {
  const c = new SampleClock(12_000);
  assert.equal(c.utc(0), null);
  c.observe(12_000, 1001.2); // 1 s of samples, 0.2 s late
  c.observe(24_000, 1002.05);
  assert.ok(Math.abs(c.utc(0)! - 1000.05) < 1e-9);
  assert.ok(Math.abs(c.index(1000.05 + 0.5)! - 6000) < 1e-6);
  // a jump forward of more than a second is a new time base
  c.observe(36_000, 1010);
  assert.ok(Math.abs(c.utc(36_000)! - 1010) < 1e-9);
});

/** Audio whose samples are their own indices, in frames of `frame`, the
 * clock exact: sample i at UTC t0 + i / 12000. */
function run(slotter: Slotter, t0: number, frames: number, frame = 600, from = 0) {
  const clock = new SampleClock(12_000);
  const slots = [];
  for (let k = 0; k < frames; k++) {
    const first = from + k * frame;
    const audio = Float32Array.from({ length: frame }, (_, i) => first + i);
    clock.observe(first + frame, t0 + (first + frame) / 12_000);
    slots.push(...slotter.push(first, audio, clock));
  }
  return slots;
}

test("FT8 slots start on the 15 s boundaries, whole and aligned", () => {
  // the boundaries are at 1_699_999_995, 1_700_000_010, ..: listening
  // from 8 s into a slot, that one is skipped
  const t0 = 1_700_000_003;
  const slots = run(new Slotter(15, 14), t0, (40 * 12_000) / 600);
  assert.deepEqual(
    slots.map((s) => s.start),
    [1_700_000_010, 1_700_000_025],
  );
  for (const s of slots) {
    assert.equal(s.audio.length, 14 * 12_000);
    // the slot's first sample is the one at its start
    assert.equal(s.audio[0], (s.start - t0) * 12_000);
    assert.equal(s.audio[s.audio.length - 1], (s.start - t0) * 12_000 + s.audio.length - 1);
  }
});

test("a break in the samples starts over", () => {
  const s = new Slotter(7.5, 6.6);
  const t0 = 1_700_000_000;
  // boundaries at 1_700_000_002.5, 010, 017.5: 10 s give the one at 2.5 s
  assert.equal(run(s, t0, 200).length, 1);
  // samples from 11 s on: the buffer starts again, and the slot at 10 s
  // can't be whole any more (nor the one at 17.5 s, by 21 s)
  assert.equal(run(s, t0, 200, 600, 11 * 12_000).length, 0);
});

test("WSPR slots are the even minutes", () => {
  const t0 = 1_700_000_000; // 80 s into an even-minute slot
  const slots = run(new Slotter(120, 114), t0, (200 * 12_000) / 1200, 1200);
  assert.deepEqual(
    slots.map((x) => x.start % 120),
    [0],
  );
  assert.equal(slots[0].audio.length, 114 * 12_000);
});

test("rows carry the slot time and the RF frequency", () => {
  const rows = toRows(
    { ms: 1, cut: false, decodes: [{ snr: -10, dt: 0.2, hz: 1234.5, text: "CQ N0CALL FN42" }] },
    "ft8",
    14_074_000,
    1_700_000_015,
  );
  assert.deepEqual(rows, [{ t: 1_700_000_015, mode: "ft8", rf: 14_075_234.5, snr: -10, dt: 0.2, text: "CQ N0CALL FN42" }]);
});

test("modes, dials and budgets", () => {
  assert.equal(MODES.ft8.dials.find((d) => d.band === "20m")!.hz, 14_074_000);
  assert.equal(MODES.ft4.dials.find((d) => d.band === "40m")!.hz, 7_047_500);
  assert.equal(MODES.wspr.dials.find((d) => d.band === "30m")!.hz, 10_138_700);
  assert.equal(budgetMs("ft4"), 6500);
});
