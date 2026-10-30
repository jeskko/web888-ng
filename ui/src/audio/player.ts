// SPDX-License-Identifier: GPL-3.0-or-later
// Audio output: AudioContext + the sink (sink.ts) + a volume node.
// Must be started from a user gesture (browser autoplay rules).
//
// The sink runs in an AudioWorklet (its own thread) where the browser
// allows one: on HTTPS and localhost. A page over plain HTTP, as a unit on
// the local network is usually opened, gets the main-thread fallback: a
// ScriptProcessorNode (deprecated, but in every browser and allowed
// without HTTPS) with bigger blocks and a deeper jitter buffer, as the
// waterfall shares its thread.

import { PcmSink, type SinkMessage, type SinkStats } from "./sink.ts";

export type PlayerStats = SinkStats;

/** The fallback's block (frames) and jitter buffer (ms). */
const FALLBACK_BLOCK = 4096;
const FALLBACK_TARGET_MS = 300;

export class Player {
  private ctx: AudioContext | null = null;
  private send: ((m: SinkMessage, transfer?: Transferable[]) => void) | null = null;
  private gain: GainNode | null = null;
  private rate = 0;
  private stereo: { left: Float32Array; right: Float32Array } | null = null;
  /** True when playing through the main-thread fallback (plain HTTP). */
  fallback = false;
  onStats: (s: PlayerStats) => void = () => {};

  get running(): boolean {
    return this.ctx?.state === "running";
  }

  async start(): Promise<void> {
    if (this.ctx) {
      await this.ctx.resume();
      return;
    }
    const ctx = new AudioContext({ latencyHint: "interactive" });
    const gain = ctx.createGain();
    gain.connect(ctx.destination);
    if (window.isSecureContext && ctx.audioWorklet) {
      await ctx.audioWorklet.addModule("worklet.js");
      const node = new AudioWorkletNode(ctx, "pcm-player", { numberOfOutputs: 1, outputChannelCount: [2] });
      node.connect(gain);
      node.port.onmessage = (e) => this.onStats(e.data as PlayerStats);
      this.send = (m, transfer) => node.port.postMessage(m, transfer ?? []);
    } else {
      const sink = new PcmSink(ctx.sampleRate, FALLBACK_TARGET_MS, (s) => this.onStats(s));
      const node = ctx.createScriptProcessor(FALLBACK_BLOCK, 0, 2);
      node.onaudioprocess = (e) => {
        const b = e.outputBuffer;
        sink.render([b.getChannelData(0), b.getChannelData(1)]);
      };
      node.connect(gain);
      this.send = (m) => sink.message(m);
      this.fallback = true;
    }
    this.ctx = ctx;
    this.gain = gain;
    if (this.rate) this.send({ rate: this.rate });
    this.send({ stereo: this.stereo });
    await ctx.resume();
  }

  /** Mono samples, or (`stereo`) left and right interleaved. */
  push(pcm: Float32Array, rate: number, stereo = false): void {
    if (!this.send) return;
    if (rate !== this.rate) {
      this.rate = rate;
      this.send({ rate });
    }
    if (!stereo) {
      this.send(pcm, [pcm.buffer]);
      return;
    }
    const n = pcm.length >> 1;
    const left = new Float32Array(n);
    const right = new Float32Array(n);
    for (let k = 0; k < n; k++) {
      left[k] = pcm[2 * k];
      right[k] = pcm[2 * k + 1];
    }
    this.send({ left, right }, [left.buffer, right.buffer]);
  }

  /** The pseudo-stereo filters (`stereo.ts`), or null for mono. */
  setStereo(taps: { left: Float32Array; right: Float32Array } | null): void {
    this.stereo = taps;
    this.send?.({ stereo: taps });
  }

  /** Linear volume 0..1 (smoothed to avoid clicks). */
  setVolume(v: number): void {
    if (this.ctx && this.gain) this.gain.gain.setTargetAtTime(v, this.ctx.currentTime, 0.02);
  }
}
