// SPDX-License-Identifier: GPL-3.0-or-later
// HF fax and SSTV in the browser (pure parts, unit-tested): stations to
// tune to, and the decoder's events (img/worker.ts, the web888-img crate in
// WebAssembly) applied to pictures: pixel buffers that grow row by row,
// drawn again when the timing is refined, kept in a small gallery when done.

/** The worker's events (rows point into the blob). */
export type ImgEv =
  | { k: "faxImage"; width: number; ioc: number; auto: boolean }
  | { k: "faxRows"; first: number; off: number; len: number }
  | { k: "faxRedraw" }
  | { k: "faxDone"; lines: number }
  | { k: "sstvImage"; mode: string; width: number; height: number }
  | { k: "sstvRows"; first: number; off: number; len: number }
  | { k: "sstvRedraw" }
  | { k: "sstvDone"; lines: number };

export interface ImgStatus {
  fax: { state: "idle" | "start" | "image"; lines: number; slant: number; ioc: number };
  sstv: { mode: string; lines: number; slant: number } | null;
}

export interface ImgRun extends ImgStatus {
  ev: ImgEv[];
}

/** A picture being received or done. */
export interface Picture {
  kind: "fax" | "sstv";
  /** "Martin 1", "IOC 576". */
  mode: string;
  width: number;
  /** 1 (fax: grey) or 3 (SSTV: RGB). */
  channels: number;
  /** Rows received (fax: grows; SSTV: up to `height`). */
  rows: number;
  height: number | null;
  data: Uint8Array;
  done: boolean;
  /** UTC seconds of its start, and the dial it was received on. */
  t: number;
  dial: number;
  /** Bumped on every change (redraw: rows went back). */
  version: number;
}

export interface ImgState {
  fax: Picture | null;
  sstv: Picture | null;
  gallery: Picture[];
  status: ImgStatus | null;
}

export const GALLERY = 12;

/** Fax stations: the carrier (the dial is 1.9 kHz below, USB). */
export const FAX_STATIONS: { name: string; hz: number }[] = [
  { name: "DWD 3855 kHz", hz: 3_855_000 },
  { name: "DWD 7880 kHz", hz: 7_880_000 },
  { name: "DWD 13882.5 kHz", hz: 13_882_500 },
];
/** SSTV calling frequencies (USB dial). */
export const SSTV_DIALS: { name: string; hz: number }[] = [
  { name: "SSTV 7.171 MHz", hz: 7_171_000 },
  { name: "SSTV 14.230 MHz", hz: 14_230_000 },
  { name: "SSTV 21.340 MHz", hz: 21_340_000 },
  { name: "SSTV 28.680 MHz", hz: 28_680_000 },
];
/** Fax: carrier − 1.9 kHz is the dial. */
export const FAX_OFFSET = 1900;

function picture(kind: "fax" | "sstv", mode: string, width: number, height: number | null, now: number, dial: number): Picture {
  const channels = kind === "fax" ? 1 : 3;
  return { kind, mode, width, channels, rows: 0, height, data: new Uint8Array(width * channels * (height ?? 256)), done: false, t: now, dial, version: 0 };
}

function putRows(p: Picture, first: number, bytes: Uint8Array): void {
  const row = p.width * p.channels;
  const end = first * row + bytes.length;
  if (end > p.data.length) {
    const d = new Uint8Array(Math.max(end, 2 * p.data.length));
    d.set(p.data);
    p.data = d;
  }
  p.data.set(bytes, first * row);
  p.rows = Math.max(p.rows, Math.floor(end / row));
  p.version++;
}

function finish(s: ImgState, p: Picture | null): void {
  if (!p || p.done) return;
  p.done = true;
  p.version++;
  if (p.rows > 0) s.gallery = [p, ...s.gallery].slice(0, GALLERY);
}

/** Apply a run (in place; returns s). */
export function applyImg(s: ImgState, run: ImgRun, blob: Uint8Array, now: number, dial: number): ImgState {
  for (const e of run.ev) {
    switch (e.k) {
      case "faxImage":
        finish(s, s.fax);
        s.fax = picture("fax", `IOC ${e.ioc}`, e.width, null, now, dial);
        break;
      case "sstvImage":
        finish(s, s.sstv);
        s.sstv = picture("sstv", e.mode, e.width, e.height, now, dial);
        break;
      case "faxRows":
      case "sstvRows": {
        const p = e.k === "faxRows" ? s.fax : s.sstv;
        if (p) putRows(p, e.first, blob.subarray(e.off, e.off + e.len));
        break;
      }
      case "faxRedraw":
      case "sstvRedraw": {
        const p = e.k === "faxRedraw" ? s.fax : s.sstv;
        if (p) {
          p.rows = 0;
          p.version++;
        }
        break;
      }
      case "faxDone":
        finish(s, s.fax);
        break;
      case "sstvDone":
        finish(s, s.sstv);
        break;
    }
  }
  s.status = { fax: run.fax, sstv: run.sstv };
  return s;
}

/** RGBA pixels of rows [from, to) of a picture. */
export function rgba(p: Picture, from: number, to: number): Uint8ClampedArray<ArrayBuffer> {
  const out = new Uint8ClampedArray(new ArrayBuffer((to - from) * p.width * 4));
  for (let y = from; y < to; y++) {
    for (let x = 0; x < p.width; x++) {
      const o = ((y - from) * p.width + x) * 4;
      const i = (y * p.width + x) * p.channels;
      if (p.channels === 1) {
        out[o] = out[o + 1] = out[o + 2] = p.data[i];
      } else {
        out[o] = p.data[i];
        out[o + 1] = p.data[i + 1];
        out[o + 2] = p.data[i + 2];
      }
      out[o + 3] = 255;
    }
  }
  return out;
}
