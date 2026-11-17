// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { AudioRecorder, audioRecName } from "../src/audiorec.ts";

const tone = (n: number, v = 0.5) => new Float32Array(n).fill(v);

test("mono frames to a 16-bit WAV, clipped at full scale", () => {
  const start = Date.UTC(2026, 10, 17, 9, 12, 30);
  const r = new AudioRecorder(start, 7_074_000, "usb", 10);
  assert.ok(r.push(tone(240), 12000, false));
  assert.ok(r.push(Float32Array.of(2, -2), 12000, false));
  assert.equal(r.frames, 242);
  assert.equal(r.seconds, 242 / 12000);
  const { bytes, name } = r.file();
  assert.equal(name, "2026-11-17_0912_7074k_usb_web888.wav");
  assert.equal(bytes.length, 44 + 2 * 242);
  const v = new DataView(bytes.buffer, bytes.byteOffset);
  assert.equal(v.getUint16(22, true), 1);
  assert.equal(v.getUint32(24, true), 12000);
  assert.equal(v.getInt16(44, true), 16384);
  assert.equal(v.getInt16(44 + 2 * 240, true), 32767);
  assert.equal(v.getInt16(44 + 2 * 241, true), -32767);
});

test("stops at its length, and when the format changes", () => {
  const r = new AudioRecorder(0, 1e6, "am", 0.01); // 120 frames at 12 kS/s
  assert.ok(r.push(tone(100), 12000, false));
  assert.equal(r.push(tone(100), 12000, false), false);
  assert.equal(r.frames, 120);
  const s = new AudioRecorder(0, 1e6, "sas", 10);
  assert.ok(s.push(tone(200), 12000, true));
  assert.equal(s.frames, 100);
  assert.equal(s.push(tone(200), 24000, true), false);
  assert.ok(s.ended);
  assert.equal(s.frames, 100);
  const v = new DataView(s.file().bytes.buffer);
  assert.equal(v.getUint16(22, true), 2);
});

test("silence left out when asked", () => {
  const r = new AudioRecorder(0, 1e6, "usb", 10, true);
  r.push(tone(1200, 0), 12000, false);
  r.push(tone(120), 12000, false);
  r.push(tone(1200, 0), 12000, false);
  assert.equal(r.frames, 120);
  assert.ok(Math.abs(r.skippedS - 0.2) < 1e-9);
  assert.equal(audioRecName(Date.UTC(2026, 0, 2, 3, 4), 14_070_150, "cw"), "2026-01-02_0304_14070.2k_cw_web888.wav");
});
