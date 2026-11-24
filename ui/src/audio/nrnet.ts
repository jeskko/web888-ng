// SPDX-License-Identifier: GPL-3.0-or-later
// The neural SSB voice noise reduction (nr.wasm, crates/nr-wasm): a small
// network that takes away what it hears as noise, never adds anything.
// Runs on the page's thread just before the player (about 15 M
// multiply-adds a second), on 12 kHz mono audio, 21 ms late; the decoders
// keep taking the raw audio.

interface NrExports {
  memory: WebAssembly.Memory;
  nr_buffer(n: number): number;
  nr_process(n: number): void;
  nr_floor(f: number): void;
  nr_reset(): void;
}

export const NN_RATE = 12000;

export class NeuralNr {
  private x: NrExports;

  constructor(x: NrExports) {
    this.x = x;
  }

  /** The network from `url` (nr.wasm), or null when it can't load. */
  static async load(url: string): Promise<NeuralNr | null> {
    try {
      const r = await fetch(url);
      if (!r.ok) return null;
      const { instance } = await WebAssembly.instantiate(await r.arrayBuffer(), {});
      return new NeuralNr(instance.exports as unknown as NrExports);
    } catch {
      return null;
    }
  }

  /** Strength 0..1: the share of what the network finds that goes (1:
   * all of it). */
  setStrength(s: number): void {
    this.x.nr_floor(1 - Math.min(1, Math.max(0, s)));
  }

  /** A new stream: forget the last one. */
  reset(): void {
    this.x.nr_reset();
  }

  /** `pcm` (12 kHz mono) through the network: a new array as long. */
  process(pcm: Float32Array): Float32Array {
    const p = this.x.nr_buffer(pcm.length);
    new Float32Array(this.x.memory.buffer, p, pcm.length).set(pcm);
    this.x.nr_process(pcm.length);
    // a copy: the next call may move the wasm memory
    return new Float32Array(this.x.memory.buffer, this.x.nr_buffer(pcm.length), pcm.length).slice();
  }
}
