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
  nr_power(p: number): void;
  nr_reset(): void;
  nr_weights(n: number): number;
  nr_load(n: number): number;
}

/** A model offered for comparison (nr-models.json in the UI directory,
 * none in a release): its name and weights file (an nrnet.bin). */
export interface NnModel {
  name: string;
  file: string;
  note?: string;
}

/** The models to compare, or [] when the UI directory lists none. */
export async function nnModels(): Promise<NnModel[]> {
  try {
    const r = await fetch("nr-models.json");
    if (!r.ok) return [];
    const m = (await r.json()) as NnModel[];
    return Array.isArray(m) ? m.filter((v) => typeof v?.name === "string" && typeof v?.file === "string") : [];
  } catch {
    return [];
  }
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
   * all of it); over 1 (to 3), its gain raised to that power: more of
   * what it partly takes away goes, what it keeps stays. */
  setStrength(s: number): void {
    this.x.nr_floor(1 - Math.min(1, Math.max(0, s)));
    this.x.nr_power(Math.min(3, Math.max(1, s)));
  }

  /** Other weights (from `url`, an nrnet.bin) instead of the current
   * ones; false if they don't load. */
  async loadWeights(url: string): Promise<boolean> {
    try {
      const r = await fetch(url);
      if (!r.ok) return false;
      const b = new Uint8Array(await r.arrayBuffer());
      const p = this.x.nr_weights(b.length);
      new Uint8Array(this.x.memory.buffer, p, b.length).set(b);
      return this.x.nr_load(b.length) === 1;
    } catch {
      return false;
    }
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
