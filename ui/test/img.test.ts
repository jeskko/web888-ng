// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { applyImg, GALLERY, rgba, type ImgRun, type ImgState } from "../src/img.ts";

const status = { fax: { state: "image" as const, lines: 0, slant: 0, ioc: 576 }, sstv: null };
const run = (ev: ImgRun["ev"]): ImgRun => ({ ev, ...status });
const fresh = (): ImgState => ({ fax: null, sstv: null, gallery: [], status: null });

test("fax rows grow the picture; a redraw starts it over", () => {
  const s = fresh();
  applyImg(s, run([{ k: "faxImage", width: 4, ioc: 576, auto: true }, { k: "faxRows", first: 0, off: 0, len: 8 }]), Uint8Array.from([1, 2, 3, 4, 5, 6, 7, 8]), 100, 7_878_100);
  assert.equal(s.fax!.rows, 2);
  assert.equal(s.fax!.mode, "IOC 576");
  // far past the initial buffer: it grows
  const big = new Uint8Array(4 * 1000).fill(9);
  applyImg(s, run([{ k: "faxRows", first: 2, off: 0, len: big.length }]), big, 101, 0);
  assert.equal(s.fax!.rows, 1002);
  assert.deepEqual([...s.fax!.data.subarray(0, 9)], [1, 2, 3, 4, 5, 6, 7, 8, 9]);
  applyImg(s, run([{ k: "faxRedraw" }, { k: "faxRows", first: 0, off: 0, len: 4 }]), Uint8Array.from([0, 0, 0, 0]), 102, 0);
  assert.equal(s.fax!.rows, 1);
  applyImg(s, run([{ k: "faxDone", lines: 1 }]), new Uint8Array(0), 103, 0);
  assert.ok(s.fax!.done);
  assert.equal(s.gallery.length, 1);
});

test("SSTV pictures are RGB; a new one files the old in the gallery", () => {
  const s = fresh();
  for (let i = 0; i < GALLERY + 3; i++) {
    applyImg(s, run([{ k: "sstvImage", mode: "Martin 1", width: 2, height: 2 }, { k: "sstvRows", first: 0, off: 0, len: 6 }]), Uint8Array.from([255, 0, 0, 0, 255, 0]), i, 14_230_000);
  }
  assert.equal(s.gallery.length, GALLERY);
  const px = rgba(s.sstv!, 0, 1);
  assert.deepEqual([...px], [255, 0, 0, 255, 0, 255, 0, 255]);
});

test("grey fax rows as RGBA", () => {
  const s = fresh();
  applyImg(s, run([{ k: "faxImage", width: 2, ioc: 288, auto: false }, { k: "faxRows", first: 0, off: 0, len: 2 }]), Uint8Array.from([10, 200]), 0, 0);
  assert.deepEqual([...rgba(s.fax!, 0, 1)], [10, 10, 10, 255, 200, 200, 200, 255]);
});
