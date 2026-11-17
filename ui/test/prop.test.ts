// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { bandHz, bandName, cellTitle, forecastBasis, forecastTitle, relColor, stormNote, type Forecast } from "../src/prop.ts";

const f: Forecast = {
  computed: 0,
  year: 2026,
  month: 11,
  day: null,
  ssn: 83,
  ssn_source: { kind: "noaa", fetched: 1 },
  engine: "voacap",
  power_w: 100,
  noise_dbw: -150,
  rx: { locator: "FN42aa", lat: 42, lon: -72 },
  modes: [
    { name: "SSB", snr_db: 48 },
    { name: "FT8", snr_db: 13 },
  ],
  bands: [7.1, 14.1, 26.0],
  paths: [
    {
      name: "Europe",
      lat: 50,
      lon: 10,
      km: 6000,
      bearing: 51,
      muf: Array(24).fill(20),
      fot: Array(24).fill(17),
      luf: [Array(24).fill(5), Array(24).fill(3)],
      rel: [
        [Array(24).fill(0.1), Array(24).fill(0.5), Array(24).fill(null)],
        [Array(24).fill(0.9), Array(24).fill(1), Array(24).fill(null)],
      ],
      snr: [Array(24).fill(40.4), Array(24).fill(50), Array(24).fill(null)],
      error: null,
    },
  ],
};

test("bands by name, and where to tune", () => {
  assert.equal(bandName(14.1), "20 m");
  assert.equal(bandName(3.6), "80 m");
  assert.equal(bandName(26.0), "26 MHz");
  assert.equal(bandHz(14.1), 14_200_000);
  assert.equal(bandHz(26.0), 26_000_000);
});

test("what the forecast is and how it was made", () => {
  assert.equal(forecastTitle(f), "November 2026, sunspot number 83 (NOAA's prediction)");
  assert.equal(forecastTitle({ ...f, day: 17, ssn_source: { kind: "builtin" } }), "17 November 2026, sunspot number 83 (built-in table)");
  assert.match(forecastBasis(f), /month's median; 100 W .* a rural receiver/);
  assert.match(forecastBasis({ ...f, engine: "truecast", noise_dbw: -136 }), /Truecast.*noisy/);
});

test("cells: colour and tooltip", () => {
  assert.equal(relColor(null), "transparent");
  assert.equal(relColor(1), "rgb(253,231,37)");
  const p = f.paths[0];
  assert.equal(cellTitle(f, p, 0, 0, 5), "Europe, 40 m, 05 UTC: SSB gets through on 10 % of days, SNR 40 dB in 1 Hz");
  assert.equal(cellTitle(f, p, 1, 2, 23), "Europe, 26 MHz, 23 UTC: no reliability figure for FT8");
});

test("a storm warning from Kp 5 up", () => {
  assert.equal(stormNote(null), null);
  assert.equal(stormNote({ time: "", kp: 2, max24: 4.67, fetched: 0 }), null);
  assert.match(stormNote({ time: "", kp: 3, max24: 6.33, fetched: 0 }) ?? "", /Kp reached 6.3/);
});
