// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { addSpots, choices, filterSpots, latest, placeLabels, utcTime, activityChoices, mergeSpots, spotTarget, spotTitle } from "../src/spots.ts";
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
  const l = latest(list, 50);
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

const remote = (id: string, mode = "CW", hz = 7030000): Spot => ({
  id, t: 1000, mode, reported_mode: mode, band: hz < 10000000 ? "40m" : "20m",
  hz, dial: hz, call: "OH0TEST", msg: "CQ test", source: "spothole",
  activities: ["POTA", "WWFF"], refs: [{activity: "POTA", id: "FI-0001"}],
});

test("imported CW, LSB and USB tune to RF while local digital modes keep their dial", () => {
  assert.deepEqual(spotTarget(remote("cw")), {mode: "cw", hz: 7030000});
  assert.deepEqual(spotTarget(remote("lsb", "LSB", 7100000)), {mode: "lsb", hz: 7100000});
  assert.deepEqual(spotTarget(remote("usb", "USB", 14285000)), {mode: "usb", hz: 14285000});
  assert.deepEqual(spotTarget(remote("60m", "USB", 5354000)), {mode: "usb", hz: 5354000});
  for (const mode of ["FT8", "FT4", "WSPR"]) {
    assert.deepEqual(spotTarget(sp(1000, "LOCAL", 7074000, "40m", mode)), {mode: "usb", hz: 7074000});
  }
});

test("activity membership handles dual activations and composes with source, mode and reference text", () => {
  const data = mergeSpots([sp(990, "LOCAL", 7074000, "40m")], [remote("a")], 1000);
  for (const activity of ["POTA", "WWFF"]) assert.equal(filterSpots(data, {band: "40m", mode: "CW", activity, source: "spothole", text: "fi-0001"}).length, 1);
  assert.equal(filterSpots(data, {band: "", mode: "", activity: "SOTA", text: ""}).length, 0);
  assert.equal(filterSpots(data, {band: "", mode: "", activity: "none", text: ""}).length, 1);
  assert.equal(filterSpots(data, {band: "", mode: "", source: "local", text: ""}).length, 1);
  assert.deepEqual(activityChoices(data), ["POTA", "SOTA", "WWFF"]);
});

test("snapshot merge is ordered, bounded and deduplicated, with no fictional SNR", () => {
  const a = remote("a");
  const b = {...remote("b"), t: 900};
  const merged = mergeSpots([], [a, a, b, {...remote("expired"), t: -1000}], 1000);
  assert.deepEqual(merged.map(s => s.id), ["b", "a"]);
  assert.equal(merged[0].snr, undefined);
  assert.equal(mergeSpots([], Array.from({length: 600}, (_,i) => remote(String(i))), 1000).length, 500);
});

test("external labels retain multiple bands, respect expiry, and local FT8 stays short-lived", () => {
  const a = remote("a");
  const b = remote("b", "CW", 14030000);
  const values = mergeSpots([sp(1000, "LOCAL", 7074000, "40m")], [a,b], 1000);
  assert.equal(latest(values, 1050).length, 2);
  assert.equal(latest(values, 2801).length, 0);
  assert.match(spotTitle({...remote("ssb", "LSB"), reported_mode: "SSB"}), /SSB → LSB by band convention/);
  assert.doesNotMatch(spotTitle(a), /undefined|NaN/);
});
