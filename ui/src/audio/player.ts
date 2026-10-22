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
    await ctx.resume();
  }

  push(pcm: Float32Array, rate: number): void {
    if (!this.node) return;
    if (rate !== this.rate) {
      this.rate = rate;
      this.node.port.postMessage({ rate });
    }
    this.node.port.postMessage(pcm, [pcm.buffer]);
  }

  /** Linear volume 0..1 (smoothed to avoid clicks). */
  setVolume(v: number): void {
    if (this.ctx && this.gain) this.gain.gain.setTargetAtTime(v, this.ctx.currentTime, 0.02);
  }
}
