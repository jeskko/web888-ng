// SPDX-License-Identifier: GPL-3.0-or-later
// The audio (AF) spectrum: the receiver's audio as it arrives cut into
// Hann-windowed blocks (half overlapping), each a line of levels (dB) to
// draw as a spectrum and a waterfall (pure, unit-tested).
//
// Mono audio (USB, LSB, CW, AM, FM): 0 to half the audio rate. Stereo
// I/Q (mode iq): a complex spectrum, minus to plus half the rate around
// the dial. Other stereo (SAS, the pseudo-stereo is the page's own and
// not seen here): left and right averaged.

import type { Mode } from "../proto.ts";

/** Points per block: ~23 Hz bins at 12 kS/s. */
export const AF_N = 512;

export interface AfLine {
  /** Levels (dB, a full-scale sine near 0), lowest frequency first. */
  db: Float32Array;
  /** Frequency of the first and of the last point (Hz, audio). */
  f0: number;
  f1: number;
}

/** In-place radix-2 FFT of `re`, `im` (length a power of two). */
export function fft(re: Float32Array, im: Float32Array): void {
  const n = re.length;
  for (let i = 1, j = 0; i < n; i++) {
    let bit = n >> 1;
    for (; j & bit; bit >>= 1) j ^= bit;
    j ^= bit;
    if (i < j) {
      [re[i], re[j]] = [re[j], re[i]];
      [im[i], im[j]] = [im[j], im[i]];
    }
  }
  for (let len = 2; len <= n; len <<= 1) {
    const a = (-2 * Math.PI) / len;
    const wr = Math.cos(a);
    const wi = Math.sin(a);
    for (let i = 0; i < n; i += len) {
      let cr = 1;
      let ci = 0;
      for (let k = 0; k < len / 2; k++) {
        const p = i + k;
        const q = p + len / 2;
        const tr = re[q] * cr - im[q] * ci;
        const ti = re[q] * ci + im[q] * cr;
        re[q] = re[p] - tr;
        im[q] = im[p] - ti;
        re[p] += tr;
        im[p] += ti;
        const nr = cr * wr - ci * wi;
        ci = cr * wi + ci * wr;
        cr = nr;
      }
    }
  }
}

const HANN = Float32Array.from({ length: AF_N }, (_, i) => 0.5 - 0.5 * Math.cos((2 * Math.PI * i) / AF_N));
/** A full-scale sine reads 0 dB (window gain and one-sided power). */
const GAIN = HANN.reduce((a, b) => a + b, 0) / 2;

export class AfAnalyzer {
  private re = new Float32Array(AF_N);
  private im = new Float32Array(AF_N);
  private bufI = new Float32Array(AF_N);
  private bufQ = new Float32Array(AF_N);
  private fill = 0;
  private rate = 0;
  private iq = false;

  /** Add a frame; the lines it completed. `iq`: stereo is I and Q. */
  push(pcm: Float32Array, rate: number, stereo: boolean, iq: boolean): AfLine[] {
    const complex = stereo && iq;
    if (rate !== this.rate || complex !== this.iq) {
      this.rate = rate;
      this.iq = complex;
      this.fill = 0;
    }
    const out: AfLine[] = [];
    const n = stereo ? pcm.length >> 1 : pcm.length;
    for (let k = 0; k < n; k++) {
      if (stereo) {
        const l = pcm[2 * k];
        const r = pcm[2 * k + 1];
        this.bufI[this.fill] = complex ? l : 0.5 * (l + r);
        this.bufQ[this.fill] = complex ? r : 0;
      } else {
        this.bufI[this.fill] = pcm[k];
        this.bufQ[this.fill] = 0;
      }
      if (++this.fill === AF_N) {
        out.push(this.line());
        // half overlapping
        this.bufI.copyWithin(0, AF_N / 2);
        this.bufQ.copyWithin(0, AF_N / 2);
        this.fill = AF_N / 2;
      }
    }
    return out;
  }

  private line(): AfLine {
    const { re, im } = this;
    for (let i = 0; i < AF_N; i++) {
      re[i] = this.bufI[i] * HANN[i];
      im[i] = this.bufQ[i] * HANN[i];
    }
    fft(re, im);
    const db10 = (k: number) => 10 * Math.log10((re[k] * re[k] + im[k] * im[k]) / (GAIN * GAIN) + 1e-20);
    const df = this.rate / AF_N;
    if (this.iq) {
      // -rate/2 .. +rate/2: the upper half of the bins first
      const db = new Float32Array(AF_N);
      for (let i = 0; i < AF_N; i++) db[i] = db10((i + AF_N / 2) % AF_N);
      return { db, f0: -this.rate / 2, f1: this.rate / 2 - df };
    }
    const db = new Float32Array(AF_N / 2);
    for (let i = 0; i < AF_N / 2; i++) db[i] = db10(i);
    return { db, f0: 0, f1: this.rate / 2 - df };
  }
}

/** The level the display's range starts from: the quietest fifth of the
 *  points in the passband. (Of all points, on SSB most would be the
 *  filter's stopband, far below the noise, and everything heard would
 *  overload the colours.) */
export function noiseFloor(l: AfLine, s: { mode: Mode; lo: number; hi: number } | null): number {
  const n = l.db.length;
  const df = (l.f1 - l.f0) / Math.max(1, n - 1);
  let inside: number[] = [];
  if (s) {
    const [p0, p1] = audioPassband(s.mode, s.lo, s.hi);
    for (let k = 0; k < n; k++) {
      const f = l.f0 + k * df;
      if (f >= p0 && f <= p1) inside.push(l.db[k]);
    }
  }
  if (inside.length < 8) inside = Array.from(l.db);
  inside.sort((a, b) => a - b);
  return inside[Math.floor(0.2 * inside.length)];
}

/** The passband in audio Hz, as the spectrum's axis has it. */
export function audioPassband(mode: Mode, lo: number, hi: number): [number, number] {
  if (mode === "lsb") return [-hi, -lo];
  if (mode === "usb" || mode === "cw" || mode === "iq") return [lo, hi];
  // AM, FM: the audio is the modulation, up to the passband's edge
  return [0, Math.max(Math.abs(lo), Math.abs(hi))];
}
