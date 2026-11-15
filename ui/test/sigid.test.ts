// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { CLASS_INFO, toSigRows, type SigRun } from "../src/sigid.ts";

const run: SigRun = {
  sig: [
    { id: 2, hz: 1500, lo: 1300, hi: 1700, db: 20, runs: 1, top: [["ft8", 0.9], ["ft4", 0.05], ["something-new", 0.01]] },
    { id: 1, hz: 700, lo: 680, hi: 720, db: 30, runs: 0, top: [] },
  ],
};

test("sigid: rows by RF, the LSB edges in order, unknown classes dropped", () => {
  const usb = toSigRows(run, (a) => 7_074_000 + a);
  assert.deepEqual(usb.map((r) => r.id), [1, 2]);
  assert.equal(usb[1].rf, 7_075_500);
  assert.deepEqual(usb[1].top.map((t) => t.cls), ["ft8", "ft4"]);
  assert.equal(usb[0].top.length, 0);
  const lsb = toSigRows(run, (a) => 7_100_000 - a);
  assert.deepEqual(lsb.map((r) => r.id), [2, 1]);
  assert.ok(lsb[0].rfLo < lsb[0].rfHi);
  assert.equal(lsb[0].rfLo, 7_098_300);
});

test("sigid: every class has a label; the decoders opened exist", () => {
  for (const [c, v] of Object.entries(CLASS_INFO)) {
    assert.ok(v.label.length > 0, c);
    assert.ok([null, "cw", "rtty", "psk", "img", "ft8", "ft4", "wspr", "ssb", "am"].includes(v.opens), c);
  }
});
