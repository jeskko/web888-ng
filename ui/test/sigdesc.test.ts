// SPDX-License-Identifier: GPL-3.0-or-later
// "What is this?" on real analyses (test/fixtures/sigdesc.json: the JSON
// of crates/sigid-wasm's example anjson on synthetic signals of each
// mode (tools/ml/sigid/modes.py) and on recordings of AM, SSB and the
// DWD's RTTY).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { askPlan, describe, features, where, type SigAn } from "../src/sigdesc.ts";
import type { BandPlan } from "../src/markers.ts";

const fx = JSON.parse(readFileSync(new URL("./fixtures/sigdesc.json", import.meta.url), "utf8")) as Record<string, { an: SigAn | null }>;
const an = (k: string) => fx[k].an!;
const usb = { rf: (a: number) => 7_000_000 + a, plan: null, markers: [] };

const cases: [string, RegExp, string | null, string][] = [
  ["rtty", /^RTTY 45\.45 Bd, 170 Hz shift$/, "fsk", "sure"],
  ["sitor", /^SITOR-B/, "fsk", "sure"],
  // on air (2026-10-22, 10100.8 kHz): selective fading, 446 Hz measured
  ["dwd", /^RTTY 50 Bd, 450 Hz shift \(as the DWD/, "fsk", "sure"],
  ["psk31", /^PSK31 \(BPSK, 31\.25 Bd\)$/, "psk31", "sure"],
  ["cw", /^CW \(Morse\), about 2\d WPM$/, "cw", "sure"],
  ["carrier", /unmodulated carrier/, "carrier", "sure"],
  ["stanag", /^STANAG 4285/, "stanag4285", "sure"],
  ["mil110a", /^2400 Bd serial-tone/, "mil110a", "sure"],
  ["hfdl", /^HFDL/, "hfdl", "sure"],
  ["ale", /^2G ALE/, "ale", "sure"],
  ["fax", /^HF fax/, "fax", "sure"],
  ["am", /^AM \(a carrier with both sidebands\)$/, "am", "sure"],
  ["amUsb", /sidebands on one side/, "am", "likely"],
  ["ssb", /^Speech \(SSB voice\)$/, "ssb", "likely"],
];

for (const [k, title, cls, sure] of cases) {
  test(`sigdesc: ${k}`, () => {
    const d = describe(an(k), usb);
    assert.match(d.title, title, `${k}: ${d.title} | ${d.text.join(" ")}`);
    assert.equal(d.cls, cls, k);
    assert.equal(d.sure, sure, `${k}: ${d.title}`);
    assert.ok(d.text.length >= 1, k);
    assert.ok(d.facts.length >= 5, k);
    for (const s of [d.title, ...d.text, ...d.facts.flat()]) assert.ok(!/NaN|undefined|Infinity/.test(s), `${k}: ${s}`);
  });
}

test("sigdesc: the measured figures say what an analyst reads off them", () => {
  const r = features(an("rtty"));
  assert.ok(r.shift! > 160 && r.shift! < 180, `${r.shift}`);
  assert.ok(Math.abs(r.baud! - 45.45) < 0.5);
  assert.equal(features(an("psk31")).pskOrder, 2);
  assert.ok(features(an("ssb")).speech);
  assert.ok(!features(an("ssb")).am);
  assert.ok(features(an("am")).am);
  assert.ok(features(an("amUsb")).amOneSide && !features(an("amUsb")).carrier);
  const s = describe(an("ssb"), usb);
  // the identifier said AM; the figures (no carrier) say otherwise
  assert.ok(an("ssb").top[0][0] === "am");
  assert.ok(s.text.some((x) => /syllable/.test(x)), s.text.join(" "));
  assert.ok(describe(an("stanag"), usb).text.some((x) => /repeats every 10[67]/.test(x)));
  assert.equal(fx.noise.an, null);
});

test("sigdesc: LSB maps edges and drift to RF; the band plan and the labels", () => {
  const lsb = { rf: (a: number) => 3_600_000 - a, plan: null, markers: [] };
  const d = describe(an("rtty"), lsb);
  assert.ok(d.rfLo < d.rfHi);
  assert.ok(Math.abs(d.rf - (3_600_000 - an("rtty").centre)) < 1);
  const plan: BandPlan = {
    region: 1,
    title: "test",
    amateur: [{ name: "80 m", lo: 3_500_000, hi: 3_800_000, segments: [{ lo: 3_570_000, hi: 3_600_000, use: "digi" }] }],
    services: [{ name: "Maritime", service: "maritime", lo: 4_063_000, hi: 4_438_000 }],
  };
  const m = { hz: 4_209_500, name: "NAVTEX", mode: "usb" as const, emission: "fsk", src: "utility" };
  assert.match(where(3_590_000, 3_589_800, 3_590_200, { ...lsb, plan, markers: [] }).join(" "), /80 m amateur band, its narrow-band digital segment/);
  const w = where(4_209_500, 4_209_400, 4_209_600, { ...lsb, plan, markers: [m] }).join(" ");
  assert.match(w, /Maritime band \(maritime\)/);
  assert.match(w, /Listed near it: NAVTEX \(4 209\.500 kHz, FSK\)/);
});

test("sigdesc: a range in the passband is measured where it is; else the tuning that hears it", () => {
  const usb = { mode: "usb", hz: 7_040_000, lo: 300, hi: 2700, pitch: 700 };
  const band = { lo: 325, hi: 2675 };
  assert.deepEqual(askPlan(usb, band, 7_041_000, 7_041_400), { audio: [1000, 1400] });
  const lsb = { mode: "lsb", hz: 3_600_000, lo: -2700, hi: -300, pitch: 700 };
  assert.deepEqual(askPlan(lsb, { lo: 325, hi: 2675 }, 3_598_600, 3_599_000), { audio: [1000, 1400] });
  // outside: tuned so it sits in the middle, the sideband kept
  const p = askPlan(lsb, { lo: 325, hi: 2675 }, 3_590_000, 3_590_400);
  assert.ok("tune" in p);
  if ("tune" in p) {
    assert.equal(p.tune.mode, "lsb");
    assert.ok(p.tune.lo < 0 && p.tune.hi < 0, "LSB's passband is below the dial");
    const mid = p.tune.dial - (-(p.tune.lo + p.tune.hi) / 2);
    assert.ok(Math.abs(mid - 3_590_200) < 2, `${mid}`);
  }
  // wider than the passband: widened, at most 5 kHz
  const w = askPlan(usb, band, 7_050_000, 7_054_000);
  assert.ok("tune" in w && w.tune.hi - w.tune.lo === 4400 && w.tune.dial + (w.tune.lo + w.tune.hi) / 2 === 7_052_000);
  // AM: USB, the identifier doesn't read AM's audio
  const am = askPlan({ mode: "am", hz: 6_000_000, lo: -4500, hi: 4500, pitch: 700 }, null, 5_999_000, 6_001_000);
  assert.ok("tune" in am && am.tune.mode === "usb" && am.tune.hi - am.tune.lo === 2600);
});
