// SPDX-License-Identifier: GPL-3.0-or-later
// The audio codecs: mu-law as the server's table, and Opus frames
// (crates/web/tests/proto/opus-tone.json) through opus.wasm
// (scripts/build-wasm.sh) and parseFrame.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { MULAW, OpusDec } from "../src/audio/codec.ts";
import { FLAG_MULAW, FLAG_OPUS, KIND_AUDIO, parseFrame, type AudioFrame } from "../src/proto.ts";

test("mu-law: the server's codes", () => {
  // crates/web/src/codec.rs mulaw_decode
  assert.equal(MULAW[0xff], 0);
  assert.ok(MULAW[0x7f] === 0); // -0: the negative zero code
  assert.equal(MULAW[0x00], -32124 / 32768);
  assert.equal(MULAW[0x80], 32124 / 32768);
  // positive codes: falling level from 0x80 to 0xff
  for (let c = 0x81; c <= 0xff; c++) assert.ok(MULAW[c] < MULAW[c - 1]);
});

test("a mu-law frame parses to its samples", () => {
  const b = new Uint8Array(16 + 3);
  const v = new DataView(b.buffer);
  b[0] = KIND_AUDIO;
  b[1] = FLAG_MULAW;
  v.setUint16(2, 3, true);
  v.setUint32(8, 12000, true);
  b.set([0xff, 0x80, 0x00], 16);
  const f = parseFrame(b.buffer) as AudioFrame;
  assert.deepEqual([...f.pcm], [0, 32124 / 32768, -32124 / 32768]);
  assert.equal(f.bytes, 19);
});

test("Opus frames decode to the tone", async () => {
  const wasm = readFileSync(new URL("../wasm/opus.wasm", import.meta.url));
  const { instance } = await WebAssembly.instantiate(wasm, {});
  const dec = new OpusDec(instance.exports as never);
  const fx = JSON.parse(readFileSync(new URL("../../crates/web/tests/proto/opus-tone.json", import.meta.url), "utf8"));
  const out: number[] = [];
  for (const hex of fx.frames as string[]) {
    const b = new Uint8Array(hex.match(/../g)!.map((h: string) => parseInt(h, 16)));
    const f = parseFrame(b.buffer) as AudioFrame;
    assert.ok(f.flags & FLAG_OPUS);
    assert.equal(f.pcm.length, 0);
    assert.equal(f.samples, 240);
    const pcm = dec.decode(f.packet!, f.samples, f.rate, f.stereo);
    assert.ok(pcm);
    assert.equal(pcm.length, 240);
    out.push(...pcm);
  }
  // the second half: a 1 kHz tone at 0.3 (rms 0.212), and at 1 kHz
  const tail = out.slice(out.length / 2);
  const rms = Math.sqrt(tail.reduce((a, v) => a + v * v, 0) / tail.length);
  assert.ok(Math.abs(rms / 0.212 - 1) < 0.2, `rms ${rms}`);
  let re = 0;
  let im = 0;
  tail.forEach((v, k) => {
    re += v * Math.cos((2 * Math.PI * 1000 * k) / 12000);
    im += v * Math.sin((2 * Math.PI * 1000 * k) / 12000);
  });
  const amp = (2 * Math.hypot(re, im)) / tail.length;
  assert.ok(Math.abs(amp / 0.3 - 1) < 0.2, `1 kHz at ${amp}`);
});
