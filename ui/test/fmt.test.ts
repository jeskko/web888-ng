// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { formatKhz, parseFreq, tickStep, snap, bandwidths } from "../src/fmt.ts";
import { formatHash, parseHash } from "../src/hash.ts";

test("format", () => {
  assert.equal(formatKhz(7_074_000), "7 074.000");
  assert.equal(formatKhz(14_074_512.4), "14 074.512");
  assert.equal(formatKhz(198_000), "198.000");
  assert.equal(formatKhz(5), "0.005");
});

test("parse entry", () => {
  assert.equal(parseFreq("7074"), 7_074_000);
  assert.equal(parseFreq("7074.5"), 7_074_500);
  assert.equal(parseFreq("7.074M"), 7_074_000);
  assert.equal(parseFreq("7 074 khz"), 7_074_000);
  assert.equal(parseFreq("198000hz"), 198_000);
  assert.equal(parseFreq("abc"), null);
});

test("ticks and snapping", () => {
  assert.equal(tickStep(61.44e6, 10), 1e7);
  assert.equal(tickStep(480e3, 10), 5e4);
  assert.equal(snap(7_074_004, 10), 7_074_000);
  for (const m of ["usb", "lsb", "cw", "am"] as const)
    for (const b of bandwidths(m)) assert.ok(b.lo < b.hi && Math.abs(b.lo) <= 5000 && Math.abs(b.hi) <= 5000, `${m} ${b.label}`);
});

test("hash round trip", () => {
  const s = { hz: 7_074_000, mode: "usb" as const, decim: 256, centerHz: 7_100_000 };
  assert.equal(formatHash(s), "#f=7074&m=usb&z=256&c=7100");
  assert.deepEqual(parseHash(formatHash(s)), s);
  assert.deepEqual(parseHash("#f=x&m=fm&z=3"), { hz: undefined, mode: undefined, decim: undefined, centerHz: undefined });
});
