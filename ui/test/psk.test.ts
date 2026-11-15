// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { applyPsk, PSK_LINGER, PSK_TEXT, pskName, type PskRun } from "../src/psk.ts";

const rf = (hz: number) => 14_070_000 + hz;
const run = (ev: PskRun["ev"], act: PskRun["act"]): PskRun => ({ ev, act, now: 0 });

test("locks, text and weak characters merge per signal", () => {
  let s = applyPsk([], run([{ k: "lock", id: 2, hz: 1000, baud: 31.25 }, { k: "text", id: 2, hz: 1000.2, t: 1, text: "CQ " }], [{ id: 2, hz: 1000.2, baud: 31.25, q: 0.97, open: true }]), 100, rf);
  s = applyPsk(s, run([{ k: "text", id: 2, hz: 1000.3, t: 2, text: "de", weak: "01" }], [{ id: 2, hz: 1000.3, baud: 31.25, q: 0.95, open: true }]), 101, rf);
  assert.deepEqual([s[0].rf, s[0].text, s[0].weak, s[0].q, s[0].open], [14_071_000.3, "CQ de", "00001", 0.95, true]);
  // gone from the skimmer: kept a while, then dropped
  s = applyPsk(s, run([], []), 102, rf);
  assert.equal(s[0].active, false);
  assert.equal(applyPsk(s, run([], []), 101 + PSK_LINGER + 1, rf).length, 0);
});

test("text is capped with its weak marks", () => {
  let s = applyPsk([], run([{ k: "lock", id: 1, hz: 900, baud: 62.5 }], [{ id: 1, hz: 900, baud: 62.5, q: 1, open: true }]), 0, rf);
  s = applyPsk(s, run([{ k: "text", id: 1, hz: 900, t: 0, text: "e".repeat(PSK_TEXT + 5) }], [{ id: 1, hz: 900, baud: 62.5, q: 1, open: true }]), 1, rf);
  assert.equal(s[0].text.length, PSK_TEXT);
  assert.equal(s[0].weak.length, PSK_TEXT);
});

test("names", () => {
  assert.deepEqual([31.25, 62.5, 125].map(pskName), ["PSK31", "PSK63", "PSK125"]);
});
