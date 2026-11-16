// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { IqRecorder, recName } from "../src/iqrec.ts";

const frame = (frame: number, n: number, v = 0.5, centre = 7_080_000) => ({
  frame,
  centre,
  rate: 60_000,
  iq: new Float32Array(2 * n).fill(v),
});

test("lost samples become zeros, other bins are ignored", () => {
  const r = new IqRecorder(0, 10);
  assert.ok(r.push(frame(1000, 4)));
  assert.ok(r.push(frame(1004, 4)));
  assert.ok(r.push(frame(1010, 2, -0.25))); // 2 lost
  assert.equal(r.push(frame(1012, 4, 0.5, 7_110_000)), false);
  assert.equal(r.samples, 12);
  assert.equal(r.lost, 2);
  const p = r.pcm16();
  assert.equal(p.length, 24);
  const top = Math.round(32767 * 0.891);
  assert.equal(p[0], top);
  assert.deepEqual([...p.slice(16, 20)], [0, 0, 0, 0]);
  assert.equal(p[20], -Math.round(top / 2));
});

test("stops at its length", () => {
  const r = new IqRecorder(0, 0.0001); // 6 samples at 60 kS/s
  r.push(frame(0, 4));
  r.push(frame(4, 4));
  assert.ok(r.full);
  assert.equal(r.samples, 6);
  r.push(frame(8, 4));
  assert.equal(r.samples, 6);
});

test("file names and the WAV header", () => {
  const start = Date.UTC(2026, 10, 16, 4, 12, 30);
  assert.equal(recName(start, 7_080_000, 60_000, "cs16"), "2026-11-16_0412_7080k_60k_web888.cs16");
  assert.equal(recName(start, 7_085_500, 60_000, "wav"), "2026-11-16_0412_7085.5k_60k_web888.wav");
  const r = new IqRecorder(start, 10);
  r.push(frame(0, 3));
  const raw = r.file("cs16");
  assert.equal(raw.bytes.length, 12);
  const w = r.file("wav");
  assert.equal(w.bytes.length, 44 + 12);
  const v = new DataView(w.bytes.buffer, w.bytes.byteOffset);
  assert.equal(String.fromCharCode(...w.bytes.slice(0, 4)), "RIFF");
  assert.equal(v.getUint16(22, true), 2);
  assert.equal(v.getUint32(24, true), 60_000);
  assert.equal(v.getUint32(40, true), 12);
  assert.deepEqual([...w.bytes.slice(44)], [...raw.bytes]);
});
