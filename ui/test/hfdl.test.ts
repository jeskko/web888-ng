// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { applyHfdl, gsName, hfdlCentre, hfdlChannels, HFDL_KEEP, who, type HfdlEv, type HfdlStation } from "../src/hfdl.ts";

const stations: HfdlStation[] = [
  { id: 7, name: "Shannon, Ireland", lat: 52.7, lon: -8.9, f: [8942, 6532] },
  { id: 9, name: "Barrow, Alaska", lat: 71.3, lon: -156.6, f: [6646, 8936] },
  { id: 15, name: "Al Muharraq, Bahrain", lat: 26.3, lon: 50.5, f: [6646] },
];

const ev = (t: number, dir: HfdlEv["dir"]): HfdlEv => ({ t, bps: 300, slot: "S", f: 5, snr: 4, dir, gs: dir ? 7 : null, items: [] });

test("decoded bursts are logged newest first, with their UTC", () => {
  let m = applyHfdl([], { ev: [ev(1, "down"), ev(2, ""), ev(3, "up")], bursts: 3, decoded: 2, now: 4 }, 1000, 8_942_000);
  assert.deepEqual(
    m.map((x) => [x.dir, x.utc, x.rf]),
    [
      ["up", 999, 8_942_000],
      ["down", 997, 8_942_000],
    ],
  );
  // nothing decoded: the same list
  assert.equal(applyHfdl(m, { ev: [ev(5, "")], bursts: 4, decoded: 2, now: 6 }, 1002, 8_942_000), m);
  for (let i = 0; i < HFDL_KEEP; i++) m = applyHfdl(m, { ev: [ev(i, "squitter")], bursts: 0, decoded: 0, now: i }, 2000, 0);
  assert.equal(m.length, HFDL_KEEP);
});

test("channels and names", () => {
  assert.deepEqual(hfdlChannels(stations), [
    { khz: 6532, names: ["Shannon"] },
    { khz: 6646, names: ["Barrow", "Al Muharraq"] },
    { khz: 8936, names: ["Barrow"] },
    { khz: 8942, names: ["Shannon"] },
  ]);
  assert.equal(gsName(stations, 9), "Barrow");
  assert.equal(gsName(stations, 3), "GS 3");
  assert.equal(who({ ac: 1, k: "acars", text: "", flight: "XX0001", reg: "", icao: "ABCDEF", lat: null, lon: null }), "XX0001 ABCDEF");
});

test("where the bursts sit in the audio", () => {
  // the dial on the channel: 1440 Hz
  assert.deepEqual(hfdlCentre(stations, 8_942_000, 200, 2800), [1440, 8_942_000]);
  // tuned 1 kHz low with a wide passband: 2440 Hz
  assert.deepEqual(hfdlCentre(stations, 8_941_000, 200, 3800), [2440, 8_942_000]);
  // two channels in view (8936 at 2440 Hz, 8942 at 8440): the one nearer
  // the middle
  assert.deepEqual(hfdlCentre(stations, 8_935_000, 200, 9000), [2440, 8_936_000]);
  assert.deepEqual(hfdlCentre(stations, 8_935_000, 4000, 9600), [8440, 8_942_000]);
  // no channel: the usual 1440
  assert.deepEqual(hfdlCentre(stations, 7_000_000, 200, 2800), [1440, 7_000_000]);
});
