// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { BANDS, bandOf } from "../src/bands.ts";
import { keypadHz, keypadPress } from "../src/keypad.ts";
import { MODES } from "../src/proto.ts";
import { TuneSync } from "../src/tunesync.ts";
import { angleDelta, nextWheelStep, takeClicks, WHEEL_STEPS, wheelLabel, wheelTarget } from "../src/wheel.ts";

test("wheel steps", () => {
  assert.deepEqual(WHEEL_STEPS.map(wheelLabel), ["0.1 kHz", "0.5 kHz", "1 kHz", "5 kHz", "25 kHz", "50 kHz", "100 kHz"]);
  assert.equal(nextWheelStep(100), 500);
  assert.equal(nextWheelStep(100000), 100); // wraps
  assert.equal(nextWheelStep(777), 100); // unknown: starts over
});

test("angle: the short way round", () => {
  assert.ok(Math.abs(angleDelta(0.1, 0.3) - 0.2) < 1e-9);
  // across the -PI/PI seam: +0.2, not -6.08
  assert.ok(Math.abs(angleDelta(Math.PI - 0.1, -Math.PI + 0.1) - 0.2) < 1e-9);
  assert.ok(Math.abs(angleDelta(-Math.PI + 0.1, Math.PI - 0.1) + 0.2) < 1e-9);
});

test("rotation to clicks", () => {
  const per = (2 * Math.PI) / 24;
  let r = takeClicks(0, per * 0.4);
  assert.deepEqual([r.clicks, +(r.rest / per).toFixed(3)], [0, 0.4]);
  r = takeClicks(r.rest, per * 0.7); // 1.1 clicks together
  assert.equal(r.clicks, 1);
  assert.ok(Math.abs(r.rest / per - 0.1) < 1e-9);
  r = takeClicks(0, -per * 2.5);
  assert.equal(r.clicks, -2);
  assert.ok(Math.abs(r.rest / per + 0.5) < 1e-9);
  // a whole turn is CLICKS_PER_TURN clicks
  assert.equal(takeClicks(0, 2 * Math.PI - 1e-9).clicks, 23);
});

test("clicks to frequency: the first lands on the grid", () => {
  assert.equal(wheelTarget(7_074_000, 1000, 1), 7_075_000);
  assert.equal(wheelTarget(7_074_000, 1000, -3), 7_071_000);
  assert.equal(wheelTarget(7_074_300, 1000, 1), 7_075_000); // off the grid: the next one
  assert.equal(wheelTarget(7_074_300, 1000, -1), 7_074_000);
  assert.equal(wheelTarget(7_074_300, 1000, 0), 7_074_300);
  assert.equal(wheelTarget(7_074_000, 100, 5), 7_074_500);
});

test("keypad entry", () => {
  const type = (keys: string) => [...keys].reduce(keypadPress, "");
  assert.equal(type("7074"), "7074");
  assert.equal(type("7.074"), "7.074");
  assert.equal(type("7.0.7"), "7.07"); // one point only
  assert.equal(type(".5"), "0.5");
  assert.equal(type("00"), "0");
  assert.equal(type("12⌫⌫⌫"), "");
  assert.equal(keypadPress("7074", "C"), "");
  assert.equal(type("12345678901234"), "1234567890"); // length limit
  assert.equal(keypadPress("7", "x"), "7");
});

test("keypad meaning: kHz or MHz", () => {
  assert.equal(keypadHz("7074", false), 7_074_000);
  assert.equal(keypadHz("7074.5", false), 7_074_500);
  assert.equal(keypadHz("7.074", true), 7_074_000);
  assert.equal(keypadHz("14", true), 14_000_000);
  assert.equal(keypadHz("7.", false), 7_000);
  assert.equal(keypadHz("", false), null);
  assert.equal(keypadHz(".", true), null);
});

test("bands", () => {
  assert.equal(bandOf(7_074_000)?.name, "40");
  assert.equal(bandOf(14_074_000)?.name, "20");
  assert.equal(bandOf(10_000_000), null);
  for (const b of BANDS) {
    assert.ok(b.lo < b.hz && b.hz < b.hi, b.name);
    assert.ok(MODES.includes(b.mode), b.name);
    assert.equal(bandOf(b.hz)?.name, b.name);
  }
  // sorted and not overlapping
  for (let i = 1; i < BANDS.length; i++) assert.ok(BANDS[i - 1].hi < BANDS[i].lo);
});

test("a late answer to an earlier tune doesn't put the dial back", () => {
  const t = new TuneSync();
  assert.equal(t.settle(7_074_000, 0), 7_074_000); // nothing sent: as is
  t.note(7_075_000, 10);
  t.note(7_076_000, 20);
  t.note(7_077_000, 30);
  // answers arrive late and in order: the older ones are ignored
  assert.equal(t.settle(7_075_000, 40), 7_077_000);
  assert.equal(t.settle(7_076_000, 50), 7_077_000);
  assert.equal(t.settle(7_077_000, 60), 7_077_000);
  // the server moving the dial itself (listening along) goes through
  assert.equal(t.settle(7_100_000, 70), 7_100_000);
  // old tunes are forgotten after two seconds
  assert.equal(t.settle(7_075_000, 2_500), 7_075_000);
});
