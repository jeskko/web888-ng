// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { BIN_STEP, hopTargets, MISSES, named, newSkim, nextHop, shown, visit } from "../src/sigskim.ts";

test("sigskim: the hops cover the view, the stalest bin next, near the dial first", () => {
  assert.deepEqual(hopTargets(7_000_000, 7_100_000), [7_015_000, 7_045_000, 7_075_000, 7_105_000]);
  assert.deepEqual(hopTargets(7_050_000, 7_060_000), [7_065_000]);
  const sk = newSkim();
  // nothing visited: the one nearest the dial
  assert.equal(nextHop(sk, 7_000_000, 7_100_000, 7_074_000, 0), 7_075_000);
  sk.visits.set(7_080_000, 1000);
  sk.visits.set(7_050_000, 2000);
  // 7015 and 7105 never visited: of those, the one nearer the dial
  assert.equal(nextHop(sk, 7_000_000, 7_100_000, 7_074_000, 3000), 7_105_000);
  sk.visits.set(7_110_000, 2500);
  sk.visits.set(7_020_000, 2600);
  assert.equal(nextHop(sk, 7_000_000, 7_100_000, 7_074_000, 3000), 7_075_000);
  assert.equal(BIN_STEP, 30_000);
});

test("sigskim: visits keep labels by RF, name the new ones first, drop what's gone", () => {
  const sk = newSkim();
  const c = 7_080_000;
  let ids = visit(sk, c, [[-6_000, -5_000, 20], [-1_000, 1_000, 30]], 0, 1);
  assert.equal(sk.labels.length, 2);
  // one at a time: the strongest new one
  assert.deepEqual(ids, [sk.labels.find((l) => l.db === 30)!.id]);
  named(sk, ids[0], [["ft8", 0.9], ["ft4", 0.05]], 10);
  ids = visit(sk, c, [[-6_010, -4_990, 21], [-1_020, 980, 28]], 100, 2);
  // the unnamed one first, then the named one; ids kept
  assert.equal(sk.labels.length, 2);
  assert.deepEqual(ids, [sk.labels[0].id, sk.labels[1].id]);
  assert.equal(sk.labels[1].rf, c - 20);
  named(sk, sk.labels[1].id, [["ft8", 0.7], ["ssb", 0.2]], 110);
  assert.deepEqual(sk.labels[1].probs, { ft8: 0.8, ft4: 0.025, ssb: 0.1 });
  // the weak one gone for MISSES visits: dropped; one outside the bin stays
  sk.labels.push({ id: 99, rf: 7_200_000, rfLo: 7_199_900, rfHi: 7_200_100, db: 10, probs: {}, runs: 0, seen: 0, named: 0, missed: 0 });
  for (let i = 0; i < MISSES; i++) visit(sk, c, [[-1_000, 1_000, 30]], 200 + i, 4);
  assert.deepEqual(sk.labels.map((l) => l.rf), [c, 7_200_000]);
});

test("sigskim: shown: named, not noise, sure or with its second guess", () => {
  const sk = newSkim();
  visit(sk, 7_080_000, [[-9_000, -8_000, 20], [-1_000, 1_000, 30], [5_000, 5_100, 12], [9_000, 9_100, 11]], 0, 9);
  const [a, b, c, d] = sk.labels.map((l) => l.id);
  named(sk, a, [["fsk", 0.95]], 1);
  named(sk, b, [["ssb", 0.45], ["am", 0.4]], 1);
  named(sk, c, [["noise", 0.9]], 1);
  named(sk, d, [["cw", 0.2], ["noise", 0.19]], 1);
  const s = shown(sk, 7_000_000, 7_100_000);
  assert.deepEqual(s.map((x) => [x.cls, x.sure, x.alt]), [["fsk", true, undefined], ["ssb", false, "am"]]);
  // out of the view
  assert.equal(shown(sk, 7_085_000, 7_100_000).length, 0);
});
