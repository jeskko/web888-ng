// SPDX-License-Identifier: GPL-3.0-or-later
// AudioWorklet: plays the server's PCM through the pseudo-stereo filters
// and the jitter buffer and resampler (one per ear, in lockstep: the same
// pushes and pulls keep them identical). Messages in: {rate} once, then
// Float32Array chunks (mono) or {left, right} (stereo from the server, past
// the filters); {stereo: taps or null} at any time.
// Messages out (5 Hz): ResamplerStats + playing.

import { Resampler } from "./resampler.ts";
import { StereoFir } from "./stereo.ts";

// AudioWorkletGlobalScope (not in the DOM typings)
declare const sampleRate: number;
declare class AudioWorkletProcessor {
  readonly port: MessagePort;
}
declare function registerProcessor(name: string, ctor: new () => AudioWorkletProcessor): void;

class PcmPlayer extends AudioWorkletProcessor {
  private rs: Resampler | null = null;
  private rsR: Resampler | null = null;
  private fir = new StereoFir();
  private playing = false;
  private n = 0;

  constructor() {
    super();
    this.port.onmessage = (e: MessageEvent) => {
      const d = e.data;
      if (d instanceof Float32Array) {
        if (!this.rs || !this.rsR) return;
        const [l, r] = this.fir.process(d);
        this.rs.push(l);
        this.rsR.push(r);
      } else if (d?.left instanceof Float32Array) {
        if (!this.rs || !this.rsR) return;
        this.rs.push(d.left);
        this.rsR.push(d.right);
      } else if (d && "stereo" in d) {
        this.fir.setTaps(d.stereo);
      } else if (typeof d?.rate === "number" && this.rs?.inRate !== d.rate) {
        this.rs = new Resampler(d.rate, sampleRate);
        this.rsR = new Resampler(d.rate, sampleRate);
        this.fir = new StereoFir();
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
    if (out.length > 1 && this.rsR) this.rsR.pull(out[1]);
    for (let c = 2; c < out.length; c++) out[c].set(out[0]);
    // 128-sample quanta: report about 5 times a second
    if (++this.n >= sampleRate / 128 / 5) {
      this.n = 0;
      this.port.postMessage({ ...this.rs.stats(), playing: this.playing });
    }
    return true;
  }
}

registerProcessor("pcm-player", PcmPlayer);
