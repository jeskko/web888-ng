// SPDX-License-Identifier: GPL-3.0-or-later
// The server's PCM to the sound card's blocks: the pseudo-stereo filters,
// then the jitter buffer and resampler (one per ear, in lockstep: the same
// pushes and pulls keep them identical). Pure, shared by the AudioWorklet
// (worklet.ts) and the main-thread fallback (player.ts). Messages in:
// {rate} once, then Float32Array chunks (mono) or {left, right} (stereo
// from the server, past the filters); {stereo: taps or null} at any time.

import { Resampler, type ResamplerStats } from "./resampler.ts";
import { StereoFir } from "./stereo.ts";

export interface SinkStats extends ResamplerStats {
  playing: boolean;
}

export type SinkMessage =
  | Float32Array
  | { left: Float32Array; right: Float32Array }
  | { stereo: { left: Float32Array; right: Float32Array } | null }
  | { rate: number };

export class PcmSink {
  private rs: Resampler | null = null;
  private rsR: Resampler | null = null;
  private fir = new StereoFir();
  private playing = false;
  private untilStats = 0;
  readonly outRate: number;
  readonly targetMs: number;
  private onStats: (s: SinkStats) => void;

  /**
   * @param outRate the sound card's rate
   * @param targetMs the jitter buffer's fill (larger for the fallback,
   *   whose blocks are bigger and come less regularly)
   * @param onStats called about 5 times a second
   */
  constructor(outRate: number, targetMs: number, onStats: (s: SinkStats) => void) {
    this.outRate = outRate;
    this.targetMs = targetMs;
    this.onStats = onStats;
  }

  message(d: SinkMessage): void {
    if (d instanceof Float32Array) {
      if (!this.rs || !this.rsR) return;
      const [l, r] = this.fir.process(d);
      this.rs.push(l);
      this.rsR.push(r);
    } else if ("left" in d) {
      if (!this.rs || !this.rsR) return;
      this.rs.push(d.left);
      this.rsR.push(d.right);
    } else if ("stereo" in d) {
      this.fir.setTaps(d.stereo);
    } else if (this.rs?.inRate !== d.rate) {
      this.rs = new Resampler(d.rate, this.outRate, this.targetMs);
      this.rsR = new Resampler(d.rate, this.outRate, this.targetMs);
      this.fir = new StereoFir();
    }
  }

  /** Fill one block: out[0] left, out[1] right (more channels copy left). */
  render(out: Float32Array[]): void {
    if (!this.rs) {
      for (const ch of out) ch.fill(0);
      return;
    }
    this.playing = this.rs.pull(out[0]);
    if (out.length > 1 && this.rsR) this.rsR.pull(out[1]);
    for (let c = 2; c < out.length; c++) out[c].set(out[0]);
    this.untilStats -= out[0].length;
    if (this.untilStats <= 0) {
      this.untilStats = this.outRate / 5;
      this.onStats({ ...this.rs.stats(), playing: this.playing });
    }
  }
}
