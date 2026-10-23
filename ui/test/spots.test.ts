// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { addSpots, choices, filterSpots, latest, placeLabels, utcTime } from "../src/spots.ts";
import type { Spot } from "../src/proto.ts";

const sp = (t: number, call: string, dial: number, band: string, mode = "FT8"): Spot => ({
  t,
  mode,
  band,
  dial,
  hz: dial + 1000,
  snr: -10,
  dt: 0.1,
  msg: `CQ ${call} FN42`,
  call,
  grid: "FN42",
});

const list = [
  sp(0, "K1ABC", 7074000, "40m"),
  sp(15, "K2ABC", 14074000, "20m"),
  sp(15, "K1ABC", 7074000, "40m"),
  sp(30, "OH8XX", 10136000, "30m", "FT4"),
];

test("filters, newest first", () => {
  assert.deepEqual(
    filterSpots(list, { band: "", mode: "", text: "" }).map((s) => s.t),
    [30, 15, 15, 0],
  );
  assert.equal(filterSpots(list, { band: "40m", mode: "", text: "" }).length, 2);
  assert.equal(filterSpots(list, { band: "", mode: "FT4", text: "" }).length, 1);
  assert.equal(filterSpots(list, { band: "", mode: "", text: " k2a" }).length, 1);
});

test("keeps the newest", () => {
  assert.deepEqual(
    addSpots(list, [sp(45, "N0NEW", 7074000, "40m")], 3).map((s) => s.t),
    [15, 30, 45],
  );
});

test("band and mode choices", () => {
  assert.deepEqual(choices(list, "band"), ["40m", "30m", "20m"]);
  assert.deepEqual(choices(list, "mode"), ["FT4", "FT8"]);
});

test("utc time", () => {
  assert.equal(utcTime(1792789905), "21:11:45");
});

test("latest spot per sender", () => {
  const l = latest(list, 10);
  assert.deepEqual(
    l.map((s) => [s.call, s.t]),
    [
      ["K1ABC", 15],
      ["OH8XX", 30],
      ["K2ABC", 15],
    ],
  );
});

test("labels take the lowest free row", () => {
  const p = placeLabels(
    [
      { item: "a", x: 10, width: 20 },
      { item: "b", x: 20, width: 20 },
      { item: "c", x: 30, width: 20 },
      { item: "d", x: 60, width: 20 },
    ],
    2,
  );
  assert.deepEqual(
    p.map((q) => [q.item, q.row]),
    [
      ["a", 0],
      ["b", 1],
      ["c", -1],
      ["d", 0],
    ],
  );
});
