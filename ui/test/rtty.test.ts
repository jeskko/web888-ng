// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { applyFsk, describe, navtex, RTTY_LINGER, RTTY_TEXT, textRuns, type FskP, type FskRun } from "../src/rtty.ts";

const ham: FskP = { baud: 45.45, shift: 170, lo: 830, hi: 1000, markHigh: true, framing: "5N1.5", charset: "ITA2", usos: true, msb: false, mode: "async" };
const dwd: FskP = { ...ham, baud: 50, shift: 450, markHigh: false, usos: false };
const rev: FskP = { ...ham, baud: 50, shift: 250, framing: "sync", charset: "reversals (idle)", mode: "sync" };

test("parameters in RF terms: LSB mirrors which tone is mark", () => {
  assert.equal(describe(ham, "usb"), "45.45 Bd · 170 Hz · 5N1.5 · mark upper · ITA2");
  assert.equal(describe(ham, "lsb"), "45.45 Bd · 170 Hz · 5N1.5 · mark lower · ITA2");
  assert.equal(describe(dwd, "usb"), "50 Bd · 450 Hz · 5N1.5 · mark lower · ITA2, no USOS");
  assert.equal(describe(rev, "usb"), "50 Bd · 250 Hz · reversals (idle)");
  assert.equal(describe({ ...ham, mode: "sitor", charset: "CCIR476", baud: 100 }, "usb"), "100 Bd · 170 Hz · SITOR-B · mark upper · CCIR476");
});

const rf = (hz: number) => 10_100_000 + hz;
const run = (ev: FskRun["ev"], act: FskRun["act"]): FskRun => ({ ev, act, now: 0 });

test("lock, text and identified signals merge per signal", () => {
  let s = applyFsk([], run([{ k: "lock", id: 3, hz: 915, p: ham }, { k: "text", id: 3, hz: 915, t: 1, text: "CQ CQ" }, { k: "ident", hz: 2000, p: rev }], [
    { id: 3, hz: 915, locked: true, snr: 8, fe: 0.01, p: ham },
    { id: 0, hz: 2000, locked: false, snr: 4, fe: 0, p: rev },
  ]), 100, rf);
  assert.deepEqual(s.map((x) => [x.id, x.rf, x.text, x.locked]), [[3, 10_100_915, "CQ CQ", true], [0, 10_102_000, "", false]]);
  s = applyFsk(s, run([{ k: "text", id: 3, hz: 915, t: 3, text: " DE N0CALL\n" }], [{ id: 3, hz: 915, locked: true, snr: 8, fe: 0.01, p: ham }]), 102, rf);
  assert.equal(s[0].text, "CQ CQ DE N0CALL\n");
  // the identified one left the skimmer's list: kept a while, then dropped
  assert.equal(s.find((x) => x.id === 0)!.active, false);
  s = applyFsk(s, run([], [{ id: 3, hz: 915, locked: true, snr: 8, fe: 0.01, p: ham }]), 100 + RTTY_LINGER + 1, rf);
  assert.deepEqual(s.map((x) => x.id), [3]);
});

test("text is capped per signal", () => {
  const s = applyFsk([], run([{ k: "lock", id: 1, hz: 900, p: ham }, { k: "text", id: 1, hz: 900, t: 0, text: "E".repeat(RTTY_TEXT + 9) }], []), 0, rf);
  assert.equal(s[0].text.length, RTTY_TEXT);
});

test("weak characters follow the text, capped with it", () => {
  let s = applyFsk([], run([{ k: "lock", id: 1, hz: 900, p: ham }, { k: "text", id: 1, hz: 900, t: 0, text: "CQ " }], []), 0, rf);
  s = applyFsk(s, run([{ k: "text", id: 1, hz: 900, t: 1, text: "TEST", weak: "0100" }], []), 1, rf);
  assert.deepEqual([s[0].text, s[0].weak], ["CQ TEST", "0000100"]);
  assert.deepEqual(textRuns(s[0].text, s[0].weak, 100), [
    { s: "CQ T", weak: false },
    { s: "E", weak: true },
    { s: "ST", weak: false },
  ]);
  assert.deepEqual(textRuns(s[0].text, s[0].weak, 3), [
    { s: "…", weak: false },
    { s: "E", weak: true },
    { s: "ST", weak: false },
  ]);
  s = applyFsk(s, run([{ k: "text", id: 1, hz: 900, t: 2, text: "E".repeat(RTTY_TEXT), weak: "1".repeat(RTTY_TEXT) }], []), 2, rf);
  assert.equal(s[0].weak, "1".repeat(RTTY_TEXT));
});

test("NAVTEX messages from SITOR-B text", () => {
  const t = "XXZCZC EA12\nGALE WARNING 8 BALTIC\nNNNN\nZCZC IB01 SEA AREA FORECAST\nNNNN ZCZC EA13 (cut off";
  assert.deepEqual(navtex(t), [
    { station: "E", subject: "A", number: "12", text: "GALE WARNING 8 BALTIC" },
    { station: "I", subject: "B", number: "01", text: "SEA AREA FORECAST" },
  ]);
});
