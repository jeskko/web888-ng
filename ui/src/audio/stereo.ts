// SPDX-License-Identifier: GPL-3.0-or-later
// Pseudo-stereo (doc/stereo-cw.md): each audio frequency gets a place
// between the ears, from left (below the passband's middle) to right
// (above it), so signals at different pitches separate by direction.
// SSB places them by pitch on a log scale around the passband's geometric
// middle (0.8-1 kHz for the 1.8-3 kHz filters): CW signals tuned to the
// usual pitches (500 Hz - 1.1 kHz) then spread over both sides whatever
// the filter's width, where the linear middle of a 2.7 or 3 kHz filter
// (1.5-1.8 kHz) put all of them on one side.
// Option 3, both cues: a level difference (constant power, `ildDb` at
// the sides) and a time difference (`itdS` between the ears at the
// sides). A pair of 255-tap FIRs at the audio rate, designed by frequency
// sampling (pure; the design runs on the main thread, the worklet only
// convolves).

export const TAPS = 255;
/** Both filters' common delay (samples); the mono path is delayed by the
 * same, so turning stereo on or off doesn't shift the audio in time. */
export const DELAY = (TAPS - 1) / 2;

export interface StereoSpec {
  /** Audio sample rate. */
  rate: number;
  /** Audio frequency (Hz) placed in the middle, and how far from it a
   * signal is fully at one side: Hz, or (`log`) a factor. */
  centre: number;
  width: number;
  log: boolean;
  /** Higher audio = lower RF (LSB): mirror the image, so it matches the
   * waterfall. */
  flip: boolean;
  /** Level difference at the sides (dB). */
  ildDb: number;
  /** Time difference at the sides (s). */
  itdS: number;
}

export const ILD_DB = 6;
export const ITD_S = 0.4e-3;

/** The image for a mode and passband (`lo`, `hi` relative to the dial,
 * as in the state), or null where it doesn't apply (AM). */
export function stereoSpec(mode: string, lo: number, hi: number, pitch: number, rate: number): StereoSpec | null {
  const base = { rate, ildDb: ILD_DB, itdS: ITD_S };
  // SSB: the audio band's edges as pitches, centred between them by ratio
  const ssb = (a: number, b: number) => {
    const f0 = Math.max(50, Math.min(a, b));
    const f1 = Math.max(f0 * 1.1, Math.max(a, b));
    return { centre: Math.sqrt(f0 * f1), width: Math.sqrt(f1 / f0), log: true };
  };
  switch (mode) {
    case "usb":
      return { ...base, ...ssb(lo, hi), flip: false };
    case "lsb":
      return { ...base, ...ssb(-hi, -lo), flip: true };
    case "cw":
      return {
        ...base,
        centre: pitch + (lo + hi) / 2,
        width: Math.max(25, (hi - lo) / 2),
        log: false,
        flip: false,
      };
    default:
      return null;
  }
}

/** Position of audio frequency `f`: -1 left .. +1 right. */
export function pan(s: StereoSpec, f: number): number {
  const x = s.log ? Math.log(Math.max(f, 1) / s.centre) / Math.log(s.width) : (f - s.centre) / s.width;
  const p = Math.max(-1, Math.min(1, x));
  return s.flip ? -p : p;
}

/** Complex response {re, im} of each ear at audio frequency `f`. */
export function target(s: StereoSpec, f: number): { l: [number, number]; r: [number, number] } {
  const p = pan(s, f);
  // constant power, `ildDb` between the ears at the sides
  const thMax = Math.atan(10 ** (s.ildDb / 20)) - Math.PI / 4;
  const th = Math.PI / 4 + p * thMax;
  // never louder than mono in either ear
  const norm = 1 / Math.sin(Math.PI / 4 + thMax);
  // fade out towards Nyquist (the audio stops well below it)
  const ny = s.rate / 2;
  const fade = f < 0.83 * ny ? 1 : f > 0.97 * ny ? 0 : 0.5 + 0.5 * Math.cos((Math.PI * (f - 0.83 * ny)) / (0.14 * ny));
  const gl = Math.cos(th) * norm * fade;
  const gr = Math.sin(th) * norm * fade;
  // the ear on the source's side hears it first
  const tau = (p * s.itdS) / 2;
  const d = DELAY / s.rate;
  const ph = (t: number): [number, number] => [Math.cos(-2 * Math.PI * f * t), Math.sin(-2 * Math.PI * f * t)];
  const [cl, sl] = ph(d + tau);
  const [cr, sr] = ph(d - tau);
  return { l: [gl * cl, gl * sl], r: [gr * cr, gr * sr] };
}

/** The two filters' taps. */
export function design(s: StereoSpec): { left: Float32Array; right: Float32Array } {
  const n = 2048; // frequency grid
  const left = new Float32Array(TAPS);
  const right = new Float32Array(TAPS);
  const hl: [number, number][] = [];
  const hr: [number, number][] = [];
  for (let k = 0; k < n / 2; k++) {
    const t = target(s, (k * s.rate) / n);
    hl.push(t.l);
    hr.push(t.r);
  }
  for (let m = 0; m < TAPS; m++) {
    // real inverse DFT (Hermitian spectrum, Nyquist bin 0)
    let al = hl[0][0];
    let ar = hr[0][0];
    for (let k = 1; k < n / 2; k++) {
      const w = (2 * Math.PI * k * m) / n;
      const c = Math.cos(w);
      const si = Math.sin(w);
      al += 2 * (hl[k][0] * c - hl[k][1] * si);
      ar += 2 * (hr[k][0] * c - hr[k][1] * si);
    }
    // Blackman window over the taps
    const x = (2 * Math.PI * m) / (TAPS - 1);
    const win = 0.42 - 0.5 * Math.cos(x) + 0.08 * Math.cos(2 * x);
    left[m] = (al / n) * win;
    right[m] = (ar / n) * win;
  }
  return { left, right };
}

/** Response {gain, phase} of taps at frequency `f` (tests, checks). */
export function response(h: Float32Array, f: number, rate: number): { gain: number; phase: number } {
  let re = 0;
  let im = 0;
  for (let m = 0; m < h.length; m++) {
    const w = (-2 * Math.PI * f * m) / rate;
    re += h[m] * Math.cos(w);
    im += h[m] * Math.sin(w);
  }
  return { gain: Math.hypot(re, im), phase: Math.atan2(im, re) };
}

/** Mono in, two ears out: the filter pair, or (no taps) the mono signal
 * delayed by DELAY to both. */
export class StereoFir {
  private left: Float32Array | null = null;
  private right: Float32Array | null = null;
  /** The last TAPS - 1 inputs, then the new block. */
  private hist = new Float32Array(TAPS - 1);

  setTaps(t: { left: Float32Array; right: Float32Array } | null): void {
    this.left = t?.left ?? null;
    this.right = t?.right ?? null;
  }

  process(x: Float32Array): [Float32Array, Float32Array] {
    const h = TAPS - 1;
    const buf = new Float32Array(h + x.length);
    buf.set(this.hist);
    buf.set(x, h);
    const l = new Float32Array(x.length);
    const r = new Float32Array(x.length);
    if (this.left && this.right) {
      const tl = this.left;
      const tr = this.right;
      // y[k] = sum_m t[m] x[k - m]; buf[h + k] is x[k]
      for (let k = 0; k < x.length; k++) {
        let al = 0;
        let ar = 0;
        const top = h + k;
        for (let m = 0; m < TAPS; m++) {
          const v = buf[top - m];
          al += tl[m] * v;
          ar += tr[m] * v;
        }
        l[k] = al;
        r[k] = ar;
      }
    } else {
      for (let k = 0; k < x.length; k++) l[k] = buf[h + k - DELAY];
      r.set(l);
    }
    this.hist = buf.slice(buf.length - h);
    return [l, r];
  }
}
