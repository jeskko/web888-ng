// SPDX-License-Identifier: GPL-3.0-or-later
// Web Worker: the CW skimmer (crates/cw-wasm, cw.wasm) off the page's
// thread. In: {pcm: Float32Array, gen} (12 kHz mono), {band: [lo, hi]}
// (audio Hz), {neural: boolean}, {wide: {rate, lo, hi} | null} (IQ from a
// channelizer bin, or audio), {iq: Float32Array (I, Q interleaved), gen}
// or {reset: true}. Out: {run: CwRun, gen} after each decode run (every
// 2 s), {error} if it can't load.

import type { CwRun } from "../cw.ts";

interface Cw {
  memory: WebAssembly.Memory;
  cw_buffer(n: number): number;
  cw_push(n: number): number;
  cw_output(): number;
  cw_band(lo: number, hi: number): void;
  cw_neural(on: number): void;
  cw_reset(): void;
  cw_wide(rate: number, lo: number, hi: number): void;
  cw_buffer_iq(n: number): number;
  cw_push_iq(n: number): number;
}

const post = (m: unknown) => (self as unknown as Worker).postMessage(m);
const queue: MessageEvent[] = [];
let cw: Cw | null = null;
let failed = false;

function handle(e: MessageEvent): void {
  const x = cw!;
  const d = e.data;
  if (d?.reset) {
    x.cw_reset();
    return;
  }
  if (d && "wide" in d) {
    // IQ from a channelizer bin (rate, band from its centre), or null: audio
    if (d.wide) x.cw_wide(d.wide.rate, d.wide.lo, d.wide.hi);
    else x.cw_wide(0, 0, 0);
    return;
  }
  if (d?.iq) {
    const iq = d.iq as Float32Array;
    const n = iq.length >> 1;
    const p = x.cw_buffer_iq(n);
    new Float32Array(x.memory.buffer, p, 2 * n).set(iq);
    const m = x.cw_push_iq(n);
    if (m > 0) {
      const json = new TextDecoder().decode(new Uint8Array(x.memory.buffer, x.cw_output(), m));
      post({ run: JSON.parse(json), gen: d.gen });
    }
    return;
  }
  if (d?.band) {
    x.cw_band(d.band[0], d.band[1]);
    return;
  }
  if (typeof d?.neural === "boolean") {
    x.cw_neural(d.neural ? 1 : 0);
    return;
  }
  const pcm = d?.pcm as Float32Array | undefined;
  if (!pcm || pcm.length === 0) return;
  const ptr = x.cw_buffer(pcm.length);
  new Float32Array(x.memory.buffer, ptr, pcm.length).set(pcm);
  const n = x.cw_push(pcm.length);
  if (n > 0) {
    const json = new TextDecoder().decode(new Uint8Array(x.memory.buffer, x.cw_output(), n));
    post({ run: JSON.parse(json) as CwRun, gen: d.gen });
  }
}

self.onmessage = (e: MessageEvent) => {
  if (cw) handle(e);
  else if (!failed) queue.push(e);
};

(async () => {
  try {
    const bytes = await (await fetch("cw.wasm")).arrayBuffer();
    const { instance } = await WebAssembly.instantiate(bytes, {});
    cw = instance.exports as unknown as Cw;
    for (const e of queue.splice(0)) handle(e);
  } catch (err) {
    failed = true;
    queue.length = 0;
    post({ error: `CW decoder: ${err instanceof Error ? err.message : String(err)}` });
  }
})();
