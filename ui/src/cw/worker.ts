// SPDX-License-Identifier: GPL-3.0-or-later
// Web Worker: the CW skimmer (crates/cw-wasm, cw.wasm) off the page's
// thread. In: {pcm: Float32Array, gen} (12 kHz mono), {band: [lo, hi]}
// (audio Hz) or {reset: true}. Out: {run: CwRun, gen} after each decode
// run (every 2 s), {error} if it can't load.

import type { CwRun } from "../cw.ts";

interface Cw {
  memory: WebAssembly.Memory;
  cw_buffer(n: number): number;
  cw_push(n: number): number;
  cw_output(): number;
  cw_band(lo: number, hi: number): void;
  cw_reset(): void;
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
  if (d?.band) {
    x.cw_band(d.band[0], d.band[1]);
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
