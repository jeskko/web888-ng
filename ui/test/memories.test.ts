// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { asMemory, exportText, MEM_MAX, memoryAt, merge, parseMemories, remove, store, type Memory } from "../src/memories.ts";

const m = (hz: number, mode = "usb", name = ""): Memory => ({ hz, mode: mode as Memory["mode"], lo: 300, hi: 2700, name });

test("stored by frequency; the same frequency and mode replaced", () => {
  let l = store([], m(14_074_000, "usb", "FT8"));
  l = store(l, m(7_074_000, "usb", "FT8 40"));
  l = store(l, m(7_074_000, "lsb"));
  assert.deepEqual(
    l.map((x) => [x.hz, x.mode]),
    [
      [7_074_000, "lsb"],
      [7_074_000, "usb"],
      [14_074_000, "usb"],
    ],
  );
  l = store(l, m(7_074_000, "usb", "renamed"));
  assert.equal(l.length, 3);
  assert.equal(l[1].name, "renamed");
  l = remove(l, l[0]);
  assert.equal(l.length, 2);
  assert.equal(memoryAt(l, 7_074_005, "usb")?.name, "renamed");
  assert.equal(memoryAt(l, 7_074_005, "cw"), null);
  assert.equal(memoryAt(l, 7_074_100, "usb"), null);
});

test("at most MEM_MAX, the new one kept", () => {
  let l: Memory[] = [];
  for (let i = 0; i < MEM_MAX; i++) l = store(l, m(1_000_000 + i * 1000));
  l = store(l, m(500));
  assert.equal(l.length, MEM_MAX);
  assert.equal(l[0].hz, 500);
  l = store(l, m(40_000_000));
  assert.equal(l.length, MEM_MAX);
  assert.ok(l.some((x) => x.hz === 40_000_000));
});

test("export and import; bad entries skipped, bad files refused", () => {
  const l = [m(3_573_000, "usb", "FT8 80"), m(10_000_000, "am", "WWV")];
  assert.deepEqual(parseMemories(exportText(l)), l);
  // a bare array, a mode we don't have, a passband the wrong way round,
  // a name with control characters and too long
  const odd = JSON.stringify([
    { hz: 7e6, mode: "usb", lo: 300, hi: 2700, name: "a\u0007b" + "x".repeat(40) },
    { hz: 7e6, mode: "dsb", lo: 300, hi: 2700 },
    { hz: 7e6, mode: "usb", lo: 2700, hi: 300 },
    { hz: "7e6", mode: "usb", lo: 300, hi: 2700 },
  ]);
  const got = parseMemories(odd);
  assert.equal(got.length, 1);
  assert.equal(got[0].name, "ab" + "x".repeat(22));
  assert.throws(() => parseMemories("{}"));
  assert.throws(() => parseMemories("not json"));
  assert.equal(asMemory({ hz: 5e9, mode: "usb", lo: 0, hi: 1 }), null);
  assert.equal(merge(l, got).length, 3);
});
