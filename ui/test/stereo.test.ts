// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { DELAY, design, ILD_DB, ITD_S, response, StereoFir, stereoSpec, TAPS } from "../src/audio/stereo.ts";

const RATE = 12000;

/** Level (dB, right over left) and time (s, right ahead of left) between
 * the ears at `f`. */
function cues(t: { left: Float32Array; right: Float32Array }, f: number): { ild: number; itd: number } {
  const l = response(t.left, f, RATE);
  const r = response(t.right, f, RATE);
  let dph = r.phase - l.phase;
  while (dph > Math.PI) dph -= 2 * Math.PI;
  while (dph < -Math.PI) dph += 2 * Math.PI;
  return { ild: 20 * Math.log10(r.gain / l.gain), itd: dph / (2 * Math.PI * f) };
}

test("CW: centred on the pitch, both cues at the passband's edges", () => {
  const s = stereoSpec("cw", -250, 250, 700, RATE)!;
  const t = design(s);
  const mid = cues(t, 700);
  assert.ok(Math.abs(mid.ild) < 0.1, `centre ild ${mid.ild}`);
  assert.ok(Math.abs(mid.itd) < 5e-6, `centre itd ${mid.itd}`);
  // above the pitch: to the right, louder and earlier there. The filter
  // rounds the ramp's corners by about 50 Hz, so the full cues are just
  // outside the passband, and nearly so at its edges.
  const hi = cues(t, 1050);
  assert.ok(Math.abs(hi.ild - ILD_DB) < 0.1, `ild ${hi.ild}`);
  assert.ok(Math.abs(hi.itd - ITD_S) < 10e-6, `itd ${hi.itd}`);
  const lo = cues(t, 350);
  assert.ok(Math.abs(lo.ild + ILD_DB) < 0.1, `ild ${lo.ild}`);
  assert.ok(Math.abs(lo.itd + ITD_S) < 10e-6, `itd ${lo.itd}`);
  const edge = cues(t, 950);
  assert.ok(edge.ild > 0.9 * ILD_DB && edge.itd > 0.9 * ITD_S, `edge ${edge.ild} dB ${edge.itd} s`);
  // halfway: half of each
  const q = cues(t, 825);
  assert.ok(q.ild > 2 && q.ild < 4, `ild ${q.ild}`);
  assert.ok(Math.abs(q.itd - ITD_S / 2) < 20e-6, `itd ${q.itd}`);
});

test("level is never above mono, and the centre loses little", () => {
  const t = design(stereoSpec("usb", 300, 2700, 0, RATE)!);
  for (let f = 200; f <= 3000; f += 50) {
    for (const h of [t.left, t.right]) {
      const g = response(h, f, RATE).gain;
      assert.ok(g < 1.03, `${f} Hz: ${g}`);
    }
  }
  // the middle: 900 Hz (by pitch, between 300 and 2700 Hz)
  const c = response(t.left, 900, RATE).gain;
  assert.ok(c > 0.75, `centre gain ${c}`);
});

test("LSB is mirrored, so the image follows the waterfall", () => {
  // LSB 300..2700 Hz below the dial: audio 2700 Hz is the lowest RF
  const s = stereoSpec("lsb", -2700, -300, 0, RATE)!;
  assert.ok(Math.abs(s.centre - 900) < 1e-9, `centre ${s.centre}`);
  const t = design(s);
  assert.ok(cues(t, 2650).ild < -ILD_DB + 1, "high audio = low RF = left");
  assert.ok(cues(t, 320).ild > ILD_DB - 1, "low audio = high RF = right");
});

test("no image for AM", () => {
  assert.equal(stereoSpec("am", -5000, 5000, 0, RATE), null);
});

test("stereo and mono have the same delay", () => {
  const x = new Float32Array(1000);
  x[100] = 1;
  const mono = new StereoFir();
  const [ml] = mono.process(x);
  assert.equal(ml.indexOf(1), 100 + DELAY);
  const st = new StereoFir();
  st.setTaps(design(stereoSpec("cw", -250, 250, 700, RATE)!));
  // in blocks: the history carries over
  const [l1] = st.process(x.subarray(0, 300));
  const [l2] = st.process(x.subarray(300));
  const l = new Float32Array(1000);
  l.set(l1);
  l.set(l2, 300);
  let peak = 0;
  for (let k = 1; k < l.length; k++) if (Math.abs(l[k]) > Math.abs(l[peak])) peak = k;
  assert.ok(Math.abs(peak - (100 + DELAY)) <= 3, `peak at ${peak}`);
  assert.equal(TAPS, 255);
});

test("SSB: the usual CW pitches spread around the centre whatever the filter's width", () => {
  // the presets: 300 Hz up to 2.1, 2.7, 3.0, 3.3 kHz
  for (const hi of [2100, 2700, 3000, 3300]) {
    const t = design(stereoSpec("usb", 300, hi, 0, RATE)!);
    // CW tuned to 500 Hz - 1.1 kHz: within 3.5 dB of the middle, both sides
    for (const f of [500, 700, 900, 1100]) {
      const c = cues(t, f);
      assert.ok(Math.abs(c.ild) < 3.5, `${hi}: ${f} Hz at ${c.ild.toFixed(1)} dB`);
    }
    // the edges still at the sides, low left, high right
    assert.ok(cues(t, 320).ild < -ILD_DB + 1, `${hi}: low edge`);
    assert.ok(cues(t, hi - 50).ild > ILD_DB - 1.5, `${hi}: high edge`);
  }
});
