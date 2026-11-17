// SPDX-License-Identifier: GPL-3.0-or-later
// Audio recording in the browser: the receiver's audio as it arrives
// (after the server's AGC, before this page's volume and pseudo-stereo),
// saved as a 16-bit WAV (pure, unit-tested).
//
// * The first frame sets the rate and channels; a frame with others (the
//   audio rate or a stereo mode changed) ends the recording, so the file
//   stays one format.
// * Skip silence: frames that are all but silent (squelch closed) are
//   left out, so a long watch on a quiet channel keeps only what was heard.
// * The name says when (UTC), the dial and the mode:
//   2026-11-17_0912_7074k_usb_web888.wav.

import { wavFile } from "./wav.ts";

/** Longest recording (s). */
export const AREC_MAX_S = 3600;
/** A frame whose peak is below this is silence (-80 dBFS). */
const SILENCE = 1e-4;

export class AudioRecorder {
  rate = 0;
  channels = 0;
  /** Sample frames held (one per channel set). */
  frames = 0;
  /** Seconds of silence left out. */
  skippedS = 0;
  /** The format changed: the recording has ended. */
  ended = false;
  private chunks: Int16Array[] = [];

  /** Unix ms when recording began, the dial and mode (the file's name). */
  readonly start: number;
  readonly hz: number;
  readonly mode: string;
  /** Length to stop at (s). */
  readonly maxS: number;
  readonly skipSilence: boolean;

  constructor(start: number, hz: number, mode: string, maxS = AREC_MAX_S, skipSilence = false) {
    this.start = start;
    this.hz = hz;
    this.mode = mode;
    this.maxS = maxS;
    this.skipSilence = skipSilence;
  }

  get seconds(): number {
    return this.rate ? this.frames / this.rate : 0;
  }

  get full(): boolean {
    return this.ended || (this.rate > 0 && this.frames >= this.maxS * this.rate);
  }

  /** Add a frame (interleaved if stereo); false once the recording is
   * over (full, or the format changed). */
  push(pcm: Float32Array, rate: number, stereo: boolean): boolean {
    const ch = stereo ? 2 : 1;
    if (this.rate === 0) {
      this.rate = rate;
      this.channels = ch;
    } else if (rate !== this.rate || ch !== this.channels) {
      this.ended = true;
    }
    if (this.full) return false;
    const n = Math.floor(pcm.length / ch);
    if (this.skipSilence) {
      let peak = 0;
      for (const v of pcm) peak = Math.max(peak, Math.abs(v));
      if (peak < SILENCE) {
        this.skippedS += n / rate;
        return true;
      }
    }
    const take = Math.min(n, Math.round(this.maxS * this.rate) - this.frames);
    const out = new Int16Array(take * ch);
    for (let i = 0; i < out.length; i++) out[i] = Math.round(Math.max(-1, Math.min(1, pcm[i])) * 32767);
    this.chunks.push(out);
    this.frames += take;
    return !this.full;
  }

  /** The file: its bytes and name. */
  file(): { bytes: Uint8Array; name: string } {
    const all = new Int16Array(this.frames * this.channels);
    let o = 0;
    for (const c of this.chunks) {
      all.set(c, o);
      o += c.length;
    }
    const data = new Uint8Array(all.buffer, all.byteOffset, all.byteLength);
    return { bytes: wavFile(data, this.rate || 12000, this.channels || 1), name: audioRecName(this.start, this.hz, this.mode) };
  }
}

/** 2026-11-17_0912_7074k_usb_web888.wav (UTC). */
export function audioRecName(startMs: number, hz: number, mode: string): string {
  const d = new Date(startMs);
  const p = (v: number) => String(v).padStart(2, "0");
  const when = `${d.getUTCFullYear()}-${p(d.getUTCMonth() + 1)}-${p(d.getUTCDate())}_${p(d.getUTCHours())}${p(d.getUTCMinutes())}`;
  return `${when}_${Math.round(hz / 100) / 10}k_${mode}_web888.wav`;
}
