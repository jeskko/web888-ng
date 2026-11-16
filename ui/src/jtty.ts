// SPDX-License-Identifier: GPL-3.0-or-later
// JTTY decoding in the browser (pure parts, unit-tested): the common dial
// frequencies, the receiver's audio as 12 kHz mono for the decoder
// (jtty/worker.ts, the web888-jtty crate in WebAssembly), and its results
// as rows with a time and an RF frequency.

/** Common JTTY dial frequencies (USB, Hz), preliminary as of 2026-10
 * (dxzone.com "JTTY frequencies"); 20 m is the main meeting place. */
export const JTTY_DIALS: { band: string; hz: number }[] = [
  { band: "160m", hz: 1_838_000 },
  { band: "80m", hz: 3_575_000 },
  { band: "40m", hz: 7_090_000 },
  { band: "30m", hz: 10_140_000 },
  { band: "20m", hz: 14_090_000 },
  { band: "17m", hz: 18_100_000 },
  { band: "15m", hz: 21_090_000 },
  { band: "12m", hz: 24_920_000 },
  { band: "10m", hz: 28_090_000 },
  { band: "6m", hz: 50_160_000 },
];

/** The decoder's passband in USB: tone 0 from 200 to 3000 Hz, plus the
 * signal's 127 Hz. */
export const JTTY_PASSBAND = { lo: 100, hi: 3200 };

export const JTTY_RATE = 12_000;

/** A message as the decoder reports it. */
export interface JttyDecode {
  text: string;
  /** Audio frequency of tone 0 (Hz). */
  hz: number;
  /** Start in samples since the decoder's last reset. */
  start: number;
  frames: number;
}

/** A message as listed. */
export interface JttyRow {
  /** UTC seconds of its start. */
  t: number;
  /** RF frequency of tone 0 (Hz): dial + audio frequency (USB). */
  rf: number;
  hz: number;
  text: string;
  frames: number;
}

/** Rows kept. */
export const JTTY_KEEP = 500;

/** Rows from decodes: `fed` samples went to the decoder since its reset,
 * the last of them at `now` (UTC seconds), all at dial `dial`. */
export function toRows(decodes: JttyDecode[], dial: number, fed: number, now: number): JttyRow[] {
  return decodes.map((d) => ({
    t: now - (fed - d.start) / JTTY_RATE,
    rf: dial + d.hz,
    hz: d.hz,
    text: d.text,
    frames: d.frames,
  }));
}

/** Half-band low-pass for 24 → 12 kS/s: flat to about 4.5 kHz, down
 * 70 dB from about 7.5 kHz (what folds back lands above 4.5 kHz), 63 taps
 * (Blackman-windowed sinc at a quarter of the rate). */
export const HALFBAND: Float32Array = (() => {
  const n = 63;
  const m = (n - 1) / 2;
  const h = new Float32Array(n);
  let sum = 0;
  for (let k = 0; k < n; k++) {
    const x = k - m;
    const sinc = x === 0 ? 0.5 : Math.sin((Math.PI * x) / 2) / (Math.PI * x);
    const t = (2 * Math.PI * k) / (n - 1);
    const w = 0.42 - 0.5 * Math.cos(t) + 0.08 * Math.cos(2 * t);
    h[k] = sinc * w;
    sum += h[k];
  }
  for (let k = 0; k < n; k++) h[k] /= sum;
  return h;
})();

/** The receiver's audio as 12 kHz mono: stereo frames averaged, 24 kS/s
 * low-passed and halved, other rates resampled linearly (the state carried
 * across frames). */
export class JttyFeed {
  private rate = 0;
  private pos = 0; // next output position, in input samples
  private last = 0; // the previous frame's last sample
  /** 24 kS/s: the last HALFBAND.length - 1 inputs, and whether the next
   * input is an odd one (skipped). */
  private hist = new Float32Array(HALFBAND.length - 1);
  private odd = false;

  push(pcm: Float32Array, rate: number, stereo: boolean): Float32Array {
    let mono = pcm;
    if (stereo) {
      mono = new Float32Array(pcm.length >> 1);
      for (let k = 0; k < mono.length; k++) mono[k] = 0.5 * (pcm[2 * k] + pcm[2 * k + 1]);
    } else {
      mono = pcm.slice();
    }
    if (rate === JTTY_RATE) return mono;
    if (rate !== this.rate) {
      this.rate = rate;
      this.pos = 0;
      this.last = 0;
      this.hist.fill(0);
      this.odd = false;
    }
    if (rate === 2 * JTTY_RATE) return this.halve(mono);
    const step = rate / JTTY_RATE;
    const out: number[] = [];
    // position -1 is the previous frame's last sample
    while (this.pos < mono.length - 1 + 1e-9) {
      const i = Math.floor(this.pos);
      const f = this.pos - i;
      const a = i < 0 ? this.last : mono[i];
      const b = i + 1 < mono.length ? mono[i + 1] : a;
      out.push(a + f * (b - a));
      this.pos += step;
    }
    this.pos -= mono.length;
    this.last = mono.length ? mono[mono.length - 1] : this.last;
    return Float32Array.from(out);
  }

  private halve(x: Float32Array): Float32Array {
    const h = HALFBAND;
    const k0 = h.length - 1;
    // the history, then this frame
    const buf = new Float32Array(k0 + x.length);
    buf.set(this.hist);
    buf.set(x, k0);
    const out: number[] = [];
    for (let i = 0; i < x.length; i++) {
      const odd = this.odd;
      this.odd = !odd;
      if (odd) continue;
      // output for input i (the window ending there)
      let acc = 0;
      for (let j = 0; j < h.length; j++) acc += h[j] * buf[i + j];
      out.push(acc);
    }
    this.hist = buf.slice(buf.length - k0);
    return Float32Array.from(out);
  }
}
