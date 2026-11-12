// SPDX-License-Identifier: GPL-3.0-or-later
import assert from "node:assert/strict";
import { test } from "node:test";
import { GridBook } from "../src/geo.ts";
import { bandColour, bandsOf, browserPoints, dxPoints, skimmerPoints, type DxMapSpot } from "../src/map.ts";
import type { DigiRow } from "../src/digi.ts";
import type { Spot } from "../src/proto.ts";

const spot = (t: number, msg: string, extra: Partial<Spot> = {}): Spot => ({
  t,
  mode: "FT8",
  band: "20m",
  dial: 14_074_000,
  hz: 14_075_000,
  snr: -10,
  dt: 0.1,
  msg,
  ...extra,
});
const rx: [number, number] = [65.5, 25];

test("skimmer spots: senders with a locator, now or heard before", () => {
  const book = new GridBook();
  const spots = [
    spot(100, "CQ K1ABC FN42", { call: "K1ABC", grid: "FN42" }),
    spot(115, "OH2XYZ K1ABC -12", { call: "K1ABC" }), // the same call later: placed by the book
    spot(115, "OH2XYZ W9ZZZ -03"), // never said its locator
    spot(130, "CQ G4AAA IO91", { call: "G4AAA", grid: "IO91", band: "40m", hz: 7_075_000 }),
    spot(10, "CQ JA1AAA PM95", { call: "JA1AAA", grid: "PM95" }), // too old
  ];
  const p = skimmerPoints(spots, book, rx, 50);
  assert.deepEqual(
    p.map((x) => [x.call, x.t, x.grid, x.band]),
    [
      ["K1ABC", 115, "FN42", "20m"],
      ["G4AAA", 130, "IO91", "40m"],
    ],
  );
  assert.deepEqual(p[0].pos, [42.5, -71]);
  assert.deepEqual(p[0].from, rx);
  assert.equal(p[0].spot?.msg, "OH2XYZ K1ABC -12");
  // the old spot still taught the book
  assert.equal(book.get("JA1AAA"), "PM95");
});

test("this browser's decodes", () => {
  const book = new GridBook();
  book.note("K1ABC", "FN42");
  const rows: DigiRow[] = [
    { t: 100, mode: "ft8", rf: 7_075_100, snr: -5, dt: 0, text: "OH2XYZ K1ABC R-07" },
    { t: 120, mode: "wspr", rf: 14_097_050, snr: -20, dt: 1, text: "G4AAA IO91 37", drift: 0 },
    { t: 120, mode: "ft4", rf: 7_048_000, snr: 0, dt: 0, text: "CQ NOGRID1" },
  ];
  const p = browserPoints(rows, book, rx, 0);
  assert.deepEqual(
    p.map((x) => [x.call, x.band, x.mode, x.grid]),
    [
      ["K1ABC", "40m", "FT8", "FN42"],
      ["G4AAA", "20m", "WSPR", "IO91"],
    ],
  );
});

test("DX spots with places, from their spotters", () => {
  const s = (t: number, dx: string, pos: [number, number] | null): DxMapSpot => ({
    t,
    hz: 14_020_000,
    band: "20m",
    mode: "CW",
    tune: "cw",
    dx,
    de: "N0CALL",
    note: "",
    dx_pos: pos,
    de_pos: [40, -100],
  });
  const p = dxPoints([s(100, "A1A", [10, 20]), s(200, "A1A", [10, 20]), s(150, "B2B", null), s(10, "C3C", [1, 2])], 50);
  assert.deepEqual(
    p.map((x) => [x.call, x.t, x.from]),
    [["A1A", 200, [40, -100]]],
  );
  assert.deepEqual(p[0].tune, { hz: 14_020_000, mode: "cw" });
});

test("bands and their colours", () => {
  const book = new GridBook();
  const p = skimmerPoints(
    [spot(1, "CQ A1A FN42", { call: "A1A", grid: "FN42" }), spot(1, "CQ B1B FN42", { call: "B1B", grid: "FN42", band: "40m", hz: 7e6 })],
    book,
    null,
    0,
  );
  assert.deepEqual(bandsOf(p), ["40m", "20m"]);
  assert.notEqual(bandColour("20m"), bandColour("40m"));
  assert.equal(bandColour("nonsense"), "#d0d0d0");
});
