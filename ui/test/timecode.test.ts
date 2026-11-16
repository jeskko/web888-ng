// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { applySeconds, tcPlan, tcTuning, TC_STATIONS, utcText, type TcRun } from "../src/timecode.ts";

test("the station the tuning hears, and its tone", () => {
  // DCF77 in USB at 76.5 kHz: its carrier at 1 kHz
  const p = tcPlan("auto", "usb", 76_500, 300, 2700, 600);
  assert.ok("station" in p && p.station.name === "DCF77" && p.tone === 1000 && p.carrier === 77_500);
  // 60 kHz: MSF first; WWVB when asked for
  const m = tcPlan("auto", "usb", 59_000, 300, 2700, 600);
  assert.ok("station" in m && m.station.name === "MSF");
  const w = tcPlan("WWVB", "usb", 59_000, 300, 2700, 600);
  assert.ok("station" in w && w.station.name === "WWVB");
  // LSB and CW
  const l = tcPlan("auto", "lsb", 78_300, -2700, -300, 600);
  assert.ok("station" in l && l.station.name === "DCF77" && l.tone === 800);
  const c = tcPlan("auto", "cw", 77_500, -250, 250, 600);
  assert.ok("station" in c && c.tone === 600);
  // WWV only in AM, on its carrier
  const v = tcPlan("auto", "am", 10_000_000, -5000, 5000, 600);
  assert.ok("station" in v && v.station.name === "WWV/WWVH" && v.tone === 100);
  assert.ok("error" in tcPlan("auto", "usb", 10_000_000, 300, 2700, 600));
  // out of the passband: what to do
  const e = tcPlan("DCF77", "usb", 70_000, 300, 2700, 600);
  assert.ok("error" in e && e.error.includes("77.5"));
});

test("tuning a station puts its tone where the plan finds it", () => {
  for (const s of TC_STATIONS) {
    for (const c of s.carriers) {
      const t = tcTuning(s, c);
      const p = tcPlan(s.name, t.mode, t.dial, t.lo, t.hi, 600);
      assert.ok("station" in p && p.station.name === s.name && p.carrier === c, `${s.name} ${c}`);
    }
  }
});

test("the current minute's seconds", () => {
  const run = (sec: TcRun["sec"]): TcRun => ({ sec, min: [], level: 10, now: 0 });
  let cur = applySeconds([], run([{ t: 1, s: "0", q: 0.9, i: -1 }]));
  assert.equal(cur.map((x) => x.s).join(""), "0");
  cur = applySeconds([], run([{ t: 2, s: "1", q: 0.9, i: 3 }]));
  assert.equal(cur.map((x) => x.s).join(""), "   1");
  cur = applySeconds(cur, run([{ t: 3, s: "0", q: 0.2, i: 4 }, { t: 4, s: "M", q: 0.8, i: 0 }, { t: 5, s: "1", q: 0.8, i: 1 }]));
  assert.equal(cur.map((x) => x.s).join(""), "M1");
});

test("UTC text", () => {
  assert.equal(utcText(Date.UTC(2026, 10, 16, 10, 24) / 1000), "2026-11-16 10:24");
});
