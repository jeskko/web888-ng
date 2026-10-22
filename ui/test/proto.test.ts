// SPDX-License-Identifier: GPL-3.0-or-later
// Wire format against the fixtures shared with crates/web/tests/proto.rs.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { parseFrame, type AudioFrame, type WfLine } from "../src/proto.ts";

const dir = new URL("../../crates/web/tests/proto/", import.meta.url);
const frames = JSON.parse(readFileSync(new URL("frames.json", dir), "utf8"));
const messages = JSON.parse(readFileSync(new URL("messages.json", dir), "utf8"));

function buf(hex: string): ArrayBuffer {
  const b = new Uint8Array(hex.length / 2);
  for (let i = 0; i < b.length; i++) b[i] = parseInt(hex.slice(2 * i, 2 * i + 2), 16);
  return b.buffer;
}

test("audio frame", () => {
  const f = frames.audio;
  const a = parseFrame(buf(f.hex)) as AudioFrame;
  assert.equal(a.kind, 1);
  assert.equal(a.flags, f.flags);
  assert.equal(a.seq, f.seq);
  assert.equal(a.rate, f.rate);
  assert.equal(a.rssi, f.rssi);
  assert.deepEqual([...a.pcm], f.pcm);
});

test("waterfall frame", () => {
  const f = frames.wf;
  const l = parseFrame(buf(f.hex)) as WfLine;
  assert.equal(l.kind, 2);
  for (const k of ["flags", "seq", "f0", "df", "db0", "dbStep", "time"] as const) assert.equal(l[k], f[k], k);
  assert.deepEqual([...l.levels], f.levels);
});

test("short or unknown frames are rejected", () => {
  assert.equal(parseFrame(buf(frames.wf.hex.slice(0, 60))), null);
  assert.equal(parseFrame(buf("09000000")), null);
});

test("message fixtures carry the fields the UI reads", () => {
  const byType = Object.fromEntries(messages.server.map((m: { type: string }) => [m.type, m]));
  for (const k of ["hz", "mode", "lo", "hi", "pitch", "agc", "gain_db", "wf"]) assert.ok(k in byType.state, k);
  for (const k of ["center_hz", "decim", "bins", "fps"]) assert.ok(k in byType.state.wf, k);
  for (const k of ["rf_lo", "rf_hi", "adc_hz", "wf_decim_min", "wf_decim_max", "rx_rate"]) assert.ok(k in byType.hello.radio, k);
  for (const k of ["adc_overloads", "clock_ppm", "audio_drops", "wf_drops", "hw_rate_ok"]) assert.ok(k in byType.status, k);
  for (const k of ["share", "listen", "agc_speed", "nb", "nr", "squelch", "squelch_db"]) assert.ok(k in byType.state, k);
  for (const k of ["session", "class"]) assert.ok(k in byType.hello, k);
  for (const k of ["granted", "position"]) assert.ok(k in byType.lease, k);
  for (const k of ["id", "hz", "mode"]) assert.ok(k in byType.users.users[0], k);
});
