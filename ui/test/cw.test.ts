// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { applyRun, audioBand, CW_LINGER, CW_TEXT, rfOf, type CwRun } from "../src/cw.ts";

test("the audio band of each mode's passband, inside the skirts", () => {
  assert.deepEqual(audioBand("usb", 300, 2700, 700), { lo: 325, hi: 2675 });
  assert.deepEqual(audioBand("lsb", -2700, -300, 700), { lo: 325, hi: 2675 });
  assert.deepEqual(audioBand("cw", -250, 250, 700), { lo: 475, hi: 925 });
  // a very narrow CW filter still leaves the skimmer 80 Hz
  assert.deepEqual(audioBand("cw", -25, 25, 600), { lo: 560, hi: 640 });
  assert.equal(audioBand("am", -5000, 5000, 700), null);
  assert.equal(audioBand("nbfm", -5000, 5000, 700), null);
});

test("audio frequencies as RF", () => {
  assert.equal(rfOf("usb", 14_000_000, 700, 800), 14_000_800);
  assert.equal(rfOf("lsb", 7_000_000, 700, 800), 6_999_200);
  // CW: the dial is the carrier that sounds at the pitch
  assert.equal(rfOf("cw", 14_020_000, 700, 700), 14_020_000);
  assert.equal(rfOf("cw", 14_020_000, 700, 900), 14_020_200);
});

const rf = (hz: number) => 14_000_000 + hz;
const run = (ev: CwRun["ev"], act: CwRun["act"]): CwRun => ({ ev, act, now: 0 });

test("runs append text per signal, sorted by frequency", () => {
  let s = applyRun([], run([{ id: 2, hz: 900, text: "CQ", wpm: 20, snr: 8, t: 1 }, { id: 1, hz: 600, text: "TEST", wpm: 25, snr: 5, t: 1 }], [
    { id: 1, hz: 600.5, wpm: 25.1, snr: 5 },
    { id: 2, hz: 900, wpm: 20, snr: 8 },
  ]), 100, rf);
  assert.deepEqual(s.map((x) => [x.id, x.text, x.rf]), [[1, "TEST", 14_000_600.5], [2, "CQ", 14_000_900]]);
  s = applyRun(s, run([{ id: 2, hz: 900, text: " DE", wpm: 20, snr: 8, t: 3 }], [{ id: 2, hz: 900, wpm: 20, snr: 8 }]), 102, rf);
  assert.equal(s.find((x) => x.id === 2)!.text, "CQ DE");
  // signal 1 dropped by the skimmer: listed, inactive, until it lingers out
  assert.equal(s.find((x) => x.id === 1)!.active, false);
  s = applyRun(s, run([], [{ id: 2, hz: 900, wpm: 20, snr: 8 }]), 100 + CW_LINGER + 1, rf);
  assert.deepEqual(s.map((x) => x.id), [2]);
});

test("text is capped per signal", () => {
  const long = "E".repeat(CW_TEXT + 50);
  const s = applyRun([], run([{ id: 1, hz: 700, text: long, wpm: 20, snr: 0, t: 0 }], [{ id: 1, hz: 700, wpm: 20, snr: 0 }]), 0, rf);
  assert.equal(s[0].text.length, CW_TEXT);
});
