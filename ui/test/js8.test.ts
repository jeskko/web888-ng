// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { applyJs8, heard, JS8_KEEP, type Js8Frame } from "../src/js8.ts";

const rf = (hz: number) => 7_078_000 + hz;
const fr = (hz: number, flags: number, text: string, from: string | null = null, kind: Js8Frame["kind"] = "data"): Js8Frame => ({
  hz,
  dt: 0,
  snr: -10,
  flags,
  kind,
  text,
  from,
  to: null,
  grid: null,
});

test("frames join into messages per frequency", () => {
  let m = applyJs8([], "normal", [fr(1000, 1, "N0ABC: K1ABC ", "N0ABC", "directed"), fr(1500, 3, "N0XYZ: @HB HEARTBEAT FN42 ", "N0XYZ", "heartbeat")], 100, rf);
  assert.equal(m.length, 2);
  // the next frame near 1000 Hz continues, the last closes it
  m = applyJs8(m, "normal", [fr(1001, 0, "GM ")], 115, rf);
  m = applyJs8(m, "normal", [fr(1000, 2, "TNX 73")], 130, rf);
  const a = m.find((x) => x.from === "N0ABC")!;
  assert.deepEqual([a.text, a.done, a.partial, a.utc, a.last], ["N0ABC: K1ABC GM TNX 73", true, false, 100, 130]);
  // a frame with no message open: partial
  m = applyJs8(m, "normal", [fr(2000, 0, "LO ")], 145, rf);
  assert.equal(m[0].partial, true);
  // another submode's frame at the same frequency is another message
  m = applyJs8(m, "fast", [fr(2001, 2, "X")], 150, rf);
  assert.equal(m[0].sub, "fast");
  assert.equal(m.find((x) => x.text === "LO ")!.done, false);
  // stale: closed after three periods without a frame
  m = applyJs8(m, "normal", [], 200, rf);
  assert.equal(m.find((x) => x.text === "LO ")!.done, true);
  assert.deepEqual(
    heard(m).map((h) => h.call),
    ["N0ABC", "N0XYZ"],
  );
});

test("kept messages are capped", () => {
  let m = applyJs8([], "turbo", [], 0, rf);
  for (let i = 0; i < JS8_KEEP + 10; i++) m = applyJs8(m, "turbo", [fr(500, 3, `M${i}`)], i * 6, rf);
  assert.equal(m.length, JS8_KEEP);
  assert.equal(m[0].text, `M${JS8_KEEP + 9}`);
});
