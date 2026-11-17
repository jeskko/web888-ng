// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { AF_N, AfAnalyzer, audioPassband, fft, noiseFloor } from "../src/view/afspec.ts";

test("the FFT agrees with a slow DFT", () => {
  const n = 16;
  const x = Float32Array.from({ length: n }, (_, i) => Math.sin(i * 0.7) + 0.3 * Math.cos(i * 2.1));
  const re = x.slice();
  const im = new Float32Array(n);
  fft(re, im);
  for (let k = 0; k < n; k++) {
    let a = 0;
    let b = 0;
    for (let i = 0; i < n; i++) {
      a += x[i] * Math.cos((-2 * Math.PI * k * i) / n);
      b += x[i] * Math.sin((-2 * Math.PI * k * i) / n);
    }
    assert.ok(Math.abs(a - re[k]) < 1e-4 && Math.abs(b - im[k]) < 1e-4, `bin ${k}`);
  }
});

const peak = (db: Float32Array) => db.reduce((best, v, i) => (v > db[best] ? i : best), 0);

test("a tone in mono audio: its frequency and a full-scale level", () => {
  const a = new AfAnalyzer();
  const rate = 12000;
  const pcm = Float32Array.from({ length: 3 * AF_N }, (_, i) => Math.sin((2 * Math.PI * 1000 * i) / rate));
  const lines = a.push(pcm, rate, false, false);
  // half overlapping: a line every N/2 samples once the first is full
  assert.equal(lines.length, 5);
  const l = lines[2];
  assert.equal(l.db.length, AF_N / 2);
  assert.equal(l.f0, 0);
  const f = l.f0 + peak(l.db) * (rate / AF_N);
  assert.ok(Math.abs(f - 1000) <= rate / AF_N, `${f}`);
  assert.ok(Math.abs(l.db[peak(l.db)]) < 1.5, `${l.db[peak(l.db)]} dB`);
});

test("I/Q: a tone below the dial lands left of the middle", () => {
  const a = new AfAnalyzer();
  const rate = 12000;
  const f = -1500;
  const pcm = new Float32Array(4 * AF_N);
  for (let i = 0; i < 2 * AF_N; i++) {
    pcm[2 * i] = 0.5 * Math.cos((2 * Math.PI * f * i) / rate);
    pcm[2 * i + 1] = 0.5 * Math.sin((2 * Math.PI * f * i) / rate);
  }
  const lines = a.push(pcm, rate, true, true);
  const l = lines[lines.length - 1];
  assert.equal(l.db.length, AF_N);
  assert.equal(l.f0, -6000);
  const got = l.f0 + peak(l.db) * (rate / AF_N);
  assert.ok(Math.abs(got - f) <= rate / AF_N, `${got}`);
  // stereo that isn't I/Q: averaged, so the tone is only on the right
  const s = new AfAnalyzer().push(pcm, rate, true, false);
  assert.equal(s[0].db.length, AF_N / 2);
});

test("the display's floor is the noise in the passband, not the filter's stopband", () => {
  // USB 300-2700 Hz in a 0-6 kHz spectrum: inside the noise at -60 dB,
  // outside the stopband at -150 (most of the points)
  const n = 256;
  const f1 = 6000 - 6000 / n;
  const db = Float32Array.from({ length: n }, (_, k) => {
    const f = (k * f1) / (n - 1);
    return f >= 300 && f <= 2700 ? -60 : -150;
  });
  const l = { db, f0: 0, f1 };
  assert.equal(noiseFloor(l, { mode: "usb", lo: 300, hi: 2700 }), -60);
  // LSB: the passband mirrored into positive audio
  assert.deepEqual(audioPassband("lsb", -2700, -300), [300, 2700]);
  assert.equal(noiseFloor(l, { mode: "lsb", lo: -2700, hi: -300 }), -60);
  // no passband known: all points
  assert.equal(noiseFloor(l, null), -150);
});
