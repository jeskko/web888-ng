// SPDX-License-Identifier: GPL-3.0-or-later
// The audio's codecs (server crates/web/src/codec.rs): G.711 mu-law by a
// table, Opus by opus.wasm (crates/opus-wasm, the opus1 decoder), loaded
// once by the page. Frames say which they carry (FLAG_MULAW, FLAG_OPUS).

/** Each mu-law code's sample (-1..1), as the server's `mulaw_decode`. */
export const MULAW = (() => {
  const t = new Float32Array(256);
  for (let c = 0; c < 256; c++) {
    const u = ~c & 0xff;
    const exp = (u >> 4) & 7;
    const x = ((((u & 0x0f) << 3) + 0x84) << exp) - 0x84;
    t[c] = (u & 0x80 ? -x : x) / 32768;
  }
  return t;
})();

export function mulawDecode(b: Uint8Array): Float32Array {
  const out = new Float32Array(b.length);
  for (let k = 0; k < b.length; k++) out[k] = MULAW[b[k]];
  return out;
}

interface OpusExports {
  memory: WebAssembly.Memory;
  od_open(rate: number, channels: number): number;
  od_input(n: number): number;
  od_decode(n: number, samples: number): number;
  od_output(): number;
}

/** One Opus stream's decoder (opened again when the rate or the channels
 * change). */
export class OpusDec {
  private x: OpusExports;
  private rate = 0;
  private chans = 0;

  constructor(x: OpusExports) {
    this.x = x;
  }

  /** The decoder from `url` (opus.wasm), or null when it can't load. */
  static async load(url: string): Promise<OpusDec | null> {
    try {
      const r = await fetch(url);
      if (!r.ok) return null;
      const { instance } = await WebAssembly.instantiate(await r.arrayBuffer(), {});
      return new OpusDec(instance.exports as unknown as OpusExports);
    } catch {
      return null;
    }
  }

  /** `packet` to `samples` samples (both channels' when `stereo`), or
   * null when it doesn't decode. */
  decode(packet: Uint8Array, samples: number, rate: number, stereo: boolean): Float32Array | null {
    const chans = stereo ? 2 : 1;
    if (rate !== this.rate || chans !== this.chans) {
      if (this.x.od_open(rate, chans) !== 0) {
        this.rate = 0;
        return null;
      }
      this.rate = rate;
      this.chans = chans;
    }
    const p = this.x.od_input(packet.length);
    new Uint8Array(this.x.memory.buffer, p, packet.length).set(packet);
    const got = this.x.od_decode(packet.length, samples / chans);
    if (got < 0) return null;
    // a copy: the next call may move the wasm memory
    return new Float32Array(this.x.memory.buffer, this.x.od_output(), got * chans).slice();
  }
}
