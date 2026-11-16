// SPDX-License-Identifier: GPL-3.0-or-later
// Packed waterfall lines: the server's packing (crates/web/tests/proto/
// wf-packed.json, written by crates/web/src/wfpack.rs) unpacks exactly,
// through parseFrame too.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { unpackWf } from "../src/wfpack.ts";
import { FLAG_PACKED, KIND_WF, parseFrame, WF_HEADER, type WfLine } from "../src/proto.ts";

const fx = JSON.parse(readFileSync(new URL("../../crates/web/tests/proto/wf-packed.json", import.meta.url), "utf8"));
const bytes = (hex: string) => new Uint8Array(hex.match(/../g)!.map((h) => parseInt(h, 16)));

test("the server's packed lines unpack exactly", () => {
  assert.equal(fx.lines.length, 3);
  for (const l of fx.lines) {
    const levels = bytes(l.levels);
    const packed = bytes(l.packed);
    assert.ok(packed.length < levels.length);
    assert.deepEqual(unpackWf(packed, levels.length), levels);
  }
});

test("a packed frame parses to its levels; a cut one doesn't", () => {
  const l = fx.lines[0];
  const levels = bytes(l.levels);
  const packed = bytes(l.packed);
  const frame = (p: Uint8Array) => {
    const b = new Uint8Array(WF_HEADER + p.length);
    const v = new DataView(b.buffer);
    b[0] = KIND_WF;
    b[1] = FLAG_PACKED;
    v.setUint16(2, levels.length, true);
    b.set(p, WF_HEADER);
    return b.buffer;
  };
  const f = parseFrame(frame(packed)) as WfLine;
  assert.deepEqual(f.levels, levels);
  assert.equal(f.bytes, WF_HEADER + packed.length);
  assert.equal(parseFrame(frame(packed.slice(0, packed.length >> 1))), null);
});
