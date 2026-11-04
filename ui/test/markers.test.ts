// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { covered, fetchRange, LABEL_SOURCES, markerColour, markerTitle, parseSources, ribbon, tuneTo, type BandPlan, type FreqMarker } from "../src/markers.ts";

const plan: BandPlan = {
  region: 1,
  title: "test",
  amateur: [
    {
      name: "40 m",
      lo: 7_000_000,
      hi: 7_200_000,
      segments: [
        { lo: 7_000_000, hi: 7_040_000, use: "cw" },
        { lo: 7_040_000, hi: 7_060_000, use: "digi" },
        { lo: 7_060_000, hi: 7_200_000, use: "all", note: "SSB contest preferred" },
      ],
    },
  ],
  services: [
    { name: "41 m", service: "broadcast", lo: 7_200_000, hi: 7_450_000 },
    { name: "Maritime 4 MHz", service: "maritime", lo: 4_063_000, hi: 4_438_000 },
    { name: "4 MHz ship DSC", service: "maritime", lo: 4_207_000, hi: 4_209_000, sub: true },
  ],
};

test("ribbon: the pieces in view, services under the amateur segments", () => {
  const r = ribbon(plan, 7_030_000, 7_300_000);
  assert.deepEqual(
    r.map((p) => [p.lo, p.label]),
    [
      [7_200_000, "41 m"],
      [7_000_000, "40 m"], // the band's first segment carries its name
      [7_040_000, ""],
      [7_060_000, ""],
    ],
  );
  assert.match(r[3].title, /all modes, SSB contest preferred/);
  // sub-bands come after (over) their service's band, unlabelled
  const m = ribbon(plan, 4_100_000, 4_300_000);
  assert.deepEqual(
    m.map((p) => [p.label, p.lo]),
    [
      ["Maritime 4 MHz", 4_063_000],
      ["", 4_207_000],
    ],
  );
  assert.deepEqual(ribbon(plan, 10_000_000, 11_000_000), []);
});

const ft8: FreqMarker = { hz: 14_074_000, name: "FT8", mode: "usb", emission: "ft8", category: "digital", src: "amateur" };
const fax: FreqMarker = { hz: 7_880_000, dial: 7_878_100, name: "DDK3 Pinneberg", mode: "usb", emission: "fax", category: "fax", schedule: "H24", src: "utility" };

test("labels: where a click tunes, and the hover text", () => {
  assert.deepEqual(tuneTo(ft8), { hz: 14_074_000, mode: "usb" });
  assert.deepEqual(tuneTo(fax), { hz: 7_878_100, mode: "usb" }); // the dial, not the carrier
  assert.equal(markerTitle(fax), "7 880.000 kHz DDK3 Pinneberg (FAX, USB) · H24 UTC: click to tune");
  const bc: FreqMarker = { hz: 6_070_000, name: "Channel 292", mode: "am", emission: "am", schedule: "2300-0100 Sa", note: "E to Eu D", src: "eibi" };
  assert.equal(markerTitle(bc), "6 070.000 kHz Channel 292 (AM) · 2300-0100 Sa UTC · E to Eu D: click to tune");
  assert.notEqual(markerColour(bc), markerColour(ft8));
  assert.equal(markerColour({ ...ft8, src: "own" }), "#ffd25a"); // the station's own stand out
});

test("label sources remembered", () => {
  assert.deepEqual(parseSources(null), LABEL_SOURCES.map((s) => s.id)); // all at first
  assert.deepEqual(parseSources(""), []); // all turned off
  assert.deepEqual(parseSources("utility,nonsense,own"), ["utility", "own"]);
});

test("labels are fetched around the view, again only when it leaves", () => {
  const got = fetchRange(7_000_000, 7_960_000);
  assert.deepEqual(got, [6_520_000, 8_440_000]);
  assert.ok(covered(7_200_000, 8_160_000, got)); // panned a quarter: still covered
  assert.ok(!covered(7_600_000, 8_560_000, got)); // past the end
  assert.ok(!covered(7_000_000, 7_960_000, null));
});
