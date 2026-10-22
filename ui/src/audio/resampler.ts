// SPDX-License-Identifier: GPL-3.0-or-later
// Jitter buffer + linear-interpolation resampler for the audio worklet
// (pure, unit-tested). The ratio is trimmed by the buffer fill, so the
// sound card's clock follows the server's sample clock: a fuller buffer
// is played slightly faster, an emptier one slightly slower.

export interface ResamplerStats {
  /** Buffered audio (ms at the input rate). */
  fillMs: number;
  underruns: number;
  /** Current playback ratio trim (1 = nominal). */
  trim: number;
}

export class Resampler {
  readonly inRate: number;
  readonly outRate: number;
  readonly targetMs: number;
  private buf: Float32Array;
  private head = 0; // read position (integer part)
  private frac = 0; // read position (fraction)
  private len = 0; // samples buffered from head
  private playing = false;
  private trimSmooth = 1;
  underruns = 0;

  /**
   * @param inRate input sample rate (12 000)
   * @param outRate output (AudioContext) rate
   * @param targetMs fill to start at and to steer towards
   */
  constructor(inRate: number, outRate: number, targetMs = 150, capacityMs = 2000) {
    this.inRate = inRate;
    this.outRate = outRate;
    this.targetMs = targetMs;
    this.buf = new Float32Array(Math.ceil((inRate * capacityMs) / 1000));
  }

  fillMs(): number {
    return ((this.len - this.frac) * 1000) / this.inRate;
  }

  push(x: Float32Array): void {
    const cap = this.buf.length;
    // too much queued (a stall upstream): drop the oldest
    const over = this.len + x.length - (cap - 2);
    if (over > 0) {
      this.head = (this.head + over) % cap;
      this.len -= over;
    }
    let w = (this.head + this.len) % cap;
    for (let k = 0; k < x.length; k++) {
      this.buf[w] = x[k];
      w = w + 1 === cap ? 0 : w + 1;
    }
    this.len += x.length;
  }

  /** Fill `out`; returns false when it played silence (buffering). */
  pull(out: Float32Array): boolean {
    if (!this.playing) {
      if (this.fillMs() < this.targetMs) {
        out.fill(0);
        return false;
      }
      this.playing = true;
    }
    // +-0.5 % at most; full correction at +-100 % of the target fill
    const err = (this.fillMs() - this.targetMs) / this.targetMs;
    const trim = 1 + 0.005 * Math.max(-1, Math.min(1, err));
    this.trimSmooth += 0.01 * (trim - this.trimSmooth);
    const step = (this.inRate / this.outRate) * this.trimSmooth;
    const cap = this.buf.length;
    for (let k = 0; k < out.length; k++) {
      if (this.len < 2) {
        out.fill(0, k);
        this.playing = false;
        this.underruns++;
        return false;
      }
      const a = this.buf[this.head];
      const b = this.buf[(this.head + 1) % cap];
      out[k] = a + (b - a) * this.frac;
      this.frac += step;
      const adv = Math.floor(this.frac);
      if (adv > 0) {
        this.frac -= adv;
        this.head = (this.head + adv) % cap;
        this.len -= adv;
      }
    }
    return true;
  }

  stats(): ResamplerStats {
    return { fillMs: this.fillMs(), underruns: this.underruns, trim: this.trimSmooth };
  }
}
