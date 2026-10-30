// SPDX-License-Identifier: GPL-3.0-or-later
// AudioWorklet: the sink (sink.ts) on the audio thread. Messages in are
// the sink's; out (5 Hz): SinkStats.

import { PcmSink, type SinkMessage } from "./sink.ts";

// AudioWorkletGlobalScope (not in the DOM typings)
declare const sampleRate: number;
declare class AudioWorkletProcessor {
  readonly port: MessagePort;
}
declare function registerProcessor(name: string, ctor: new () => AudioWorkletProcessor): void;

class PcmPlayer extends AudioWorkletProcessor {
  private sink = new PcmSink(sampleRate, 150, (s) => this.port.postMessage(s));

  constructor() {
    super();
    this.port.onmessage = (e: MessageEvent) => this.sink.message(e.data as SinkMessage);
  }

  process(_inputs: Float32Array[][], outputs: Float32Array[][]): boolean {
    this.sink.render(outputs[0]);
    return true;
  }
}

registerProcessor("pcm-player", PcmPlayer);
