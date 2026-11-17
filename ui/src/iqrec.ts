// SPDX-License-Identifier: GPL-3.0-or-later
// IQ recording in the browser: the channelizer bin around the dial (wide
// IQ, 16-bit frames: crates/web/src/wideiq.rs) collected for a while and
// saved as a file to study offline as heard on this receiver.
//
// * Samples lost on the way (frame numbers skip) are filled with zeros, so
//   time in the file stays time on air; the count is reported.
// * .cs16: interleaved little-endian i16 I/Q, no header (GNU Radio, numpy;
//   `web888d --backend iq` takes 480 kS/s and up, so resample by 8 to
//   replay one). .wav: the same as stereo 16-bit PCM (I left, Q right) for
//   SDR programs. Both are scaled to the recording's peak
//   (-1 dBFS); the name says when (UTC), the centre and the rate:
//   2026-11-16_0412_7080k_60k_web888.cs16.

import { wavFile } from "./wav.ts";

/** Longest recording (s): 60 kS/s complex as f32 is 480 kB/s here. */
export const REC_MAX_S = 300;
/** Longest run of lost samples filled with zeros (s); a longer one is cut short. */
const MAX_FILL_S = 5;

export interface RecFrame {
  frame: number;
  centre: number;
  rate: number;
  iq: Float32Array;
}

export type RecFormat = "cs16" | "wav";

export class IqRecorder {
  centre = 0;
  rate = 0;
  /** Complex samples held (zeros for lost ones included). */
  samples = 0;
  /** Complex samples lost on the way (filled with zeros). */
  lost = 0;
  private chunks: Float32Array[] = [];
  private next: number | null = null;
  private peak = 0;

  /** Unix ms when recording began (the file's name). */
  readonly start: number;
  /** Length to stop at (s). */
  readonly maxS: number;

  constructor(start: number, maxS = REC_MAX_S) {
    this.start = start;
    this.maxS = maxS;
  }

  get seconds(): number {
    return this.rate ? this.samples / this.rate : 0;
  }

  get full(): boolean {
    return this.rate > 0 && this.samples >= this.maxS * this.rate;
  }

  /** Add a frame; false if it isn't this recording's bin (ignored). */
  push(f: RecFrame): boolean {
    if (this.rate === 0) {
      this.centre = f.centre;
      this.rate = f.rate;
    } else if (f.centre !== this.centre || f.rate !== this.rate) return false;
    if (this.full) return true;
    if (this.next !== null && f.frame > this.next) {
      const miss = f.frame - this.next;
      const fill = Math.min(miss, Math.round(MAX_FILL_S * this.rate));
      this.lost += miss;
      this.chunks.push(new Float32Array(2 * fill));
      this.samples += fill;
    }
    const room = Math.max(0, Math.round(this.maxS * this.rate) - this.samples);
    const n = Math.min(f.iq.length / 2, room);
    const iq = n * 2 === f.iq.length ? f.iq.slice() : f.iq.slice(0, 2 * n);
    for (const v of iq) {
      const a = Math.abs(v);
      if (a > this.peak) this.peak = a;
    }
    this.chunks.push(iq);
    this.samples += n;
    this.next = f.frame + f.iq.length / 2;
    return true;
  }

  /** The samples as i16 I/Q pairs, peak at -1 dBFS. */
  pcm16(): Int16Array {
    const out = new Int16Array(2 * this.samples);
    const k = this.peak > 0 ? (32767 * 0.891) / this.peak : 0;
    let o = 0;
    for (const c of this.chunks) {
      for (let i = 0; i < c.length; i++) out[o++] = Math.round(c[i] * k);
    }
    return out;
  }

  /** The file: its bytes and name. */
  file(format: RecFormat): { bytes: Uint8Array; name: string } {
    const pcm = this.pcm16();
    const data = new Uint8Array(pcm.buffer, pcm.byteOffset, pcm.byteLength);
    const name = recName(this.start, this.centre, this.rate, format);
    if (format === "cs16") return { bytes: data, name };
    return { bytes: wavFile(data, Math.round(this.rate), 2), name };
  }
}

/** 2026-11-16_0412_7080k_60k_web888.cs16 (UTC). */
export function recName(startMs: number, centre: number, rate: number, ext: string): string {
  const d = new Date(startMs);
  const p = (v: number) => String(v).padStart(2, "0");
  const when = `${d.getUTCFullYear()}-${p(d.getUTCMonth() + 1)}-${p(d.getUTCDate())}_${p(d.getUTCHours())}${p(d.getUTCMinutes())}`;
  const k = (hz: number) => `${Math.round(hz / 100) / 10}k`;
  return `${when}_${k(centre)}_${k(rate)}_web888.${ext}`;
}
