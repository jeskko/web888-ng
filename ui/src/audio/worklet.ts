// SPDX-License-Identifier: GPL-3.0-or-later
// AudioWorklet: plays the server's PCM through the jitter buffer and
// resampler. Messages in: {rate} once, then Float32Array chunks.
// Messages out (5 Hz): ResamplerStats + playing.

import { Resampler } from "./resampler.ts";

// AudioWorkletGlobalScope (not in the DOM typings)
declare const sampleRate: number;
declare class AudioWorkletProcessor {
  readonly port: MessagePort;
}
declare function registerProcessor(name: string, ctor: new () => AudioWorkletProcessor): void;

class PcmPlayer extends AudioWorkletProcessor {
  private rs: Resampler | null = null;
  private playing = false;
  private n = 0;

  constructor() {
    super();
    this.port.onmessage = (e: MessageEvent) => {
      const d = e.data;
      if (d instanceof Float32Array) {
        this.rs?.push(d);
      } else if (typeof d?.rate === "number" && this.rs?.inRate !== d.rate) {
        this.rs = new Resampler(d.rate, sampleRate);
      }
    };
  }

  process(_inputs: Float32Array[][], outputs: Float32Array[][]): boolean {
    const out = outputs[0];
    if (!this.rs) {
      for (const ch of out) ch.fill(0);
      return true;
    }
    this.playing = this.rs.pull(out[0]);
    for (let c = 1; c < out.length; c++) out[c].set(out[0]);
    // 128-sample quanta: report about 5 times a second
    if (++this.n >= sampleRate / 128 / 5) {
      this.n = 0;
      this.port.postMessage({ ...this.rs.stats(), playing: this.playing });
    }
    return true;
  }
}

registerProcessor("pcm-player", PcmPlayer);
