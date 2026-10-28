// SPDX-License-Identifier: GPL-3.0-or-later
// Audio output: AudioContext + the pcm-player worklet + a volume node.
// Must be started from a user gesture (browser autoplay rules).

import type { ResamplerStats } from "./resampler.ts";

export interface PlayerStats extends ResamplerStats {
  playing: boolean;
}

export class Player {
  private ctx: AudioContext | null = null;
  private node: AudioWorkletNode | null = null;
  private gain: GainNode | null = null;
  private rate = 0;
  private stereo: { left: Float32Array; right: Float32Array } | null = null;
  onStats: (s: PlayerStats) => void = () => {};

  get running(): boolean {
    return this.ctx?.state === "running";
  }

  async start(): Promise<void> {
    if (this.ctx) {
      await this.ctx.resume();
      return;
    }
    if (!window.isSecureContext) {
      throw new Error("audio needs HTTPS (or localhost): open this page with https://");
    }
    const ctx = new AudioContext({ latencyHint: "interactive" });
    await ctx.audioWorklet.addModule("worklet.js");
    const node = new AudioWorkletNode(ctx, "pcm-player", { numberOfOutputs: 1, outputChannelCount: [2] });
    const gain = ctx.createGain();
    node.connect(gain).connect(ctx.destination);
    node.port.onmessage = (e) => this.onStats(e.data as PlayerStats);
    this.ctx = ctx;
    this.node = node;
    this.gain = gain;
    if (this.rate) node.port.postMessage({ rate: this.rate });
    node.port.postMessage({ stereo: this.stereo });
    await ctx.resume();
  }

  /** Mono samples, or (`stereo`) left and right interleaved. */
  push(pcm: Float32Array, rate: number, stereo = false): void {
    if (!this.node) return;
    if (rate !== this.rate) {
      this.rate = rate;
      this.node.port.postMessage({ rate });
    }
    if (!stereo) {
      this.node.port.postMessage(pcm, [pcm.buffer]);
      return;
    }
    const n = pcm.length >> 1;
    const left = new Float32Array(n);
    const right = new Float32Array(n);
    for (let k = 0; k < n; k++) {
      left[k] = pcm[2 * k];
      right[k] = pcm[2 * k + 1];
    }
    this.node.port.postMessage({ left, right }, [left.buffer, right.buffer]);
  }

  /** The pseudo-stereo filters (`stereo.ts`), or null for mono. */
  setStereo(taps: { left: Float32Array; right: Float32Array } | null): void {
    this.stereo = taps;
    this.node?.port.postMessage({ stereo: taps });
  }

  /** Linear volume 0..1 (smoothed to avoid clicks). */
  setVolume(v: number): void {
    if (this.ctx && this.gain) this.gain.gain.setTargetAtTime(v, this.ctx.currentTime, 0.02);
  }
}
