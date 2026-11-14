// SPDX-License-Identifier: GPL-3.0-or-later
// Web Worker: the FSK/RTTY skimmer (crates/fsk-wasm, fsk.wasm) off the page's
// thread. In: {pcm: Float32Array, gen} (12 kHz mono), {band: [lo, hi]}
// (audio Hz) or {reset: true}. Out: {run: FskRun, gen} on new text and
// every 2 s, {error} if it can't load.

import type { FskRun } from "../rtty.ts";

interface Fsk {
  memory: WebAssembly.Memory;
  fsk_buffer(n: number): number;
  fsk_push(n: number): number;
  fsk_output(): number;
  fsk_band(lo: number, hi: number): void;
  fsk_reset(): void;
}

const post = (m: unknown) => (self as unknown as Worker).postMessage(m);
const queue: MessageEvent[] = [];
let fsk: Fsk | null = null;
let failed = false;

function handle(e: MessageEvent): void {
  const x = fsk!;
  const d = e.data;
  if (d?.reset) {
    x.fsk_reset();
    return;
  }
  if (d?.band) {
    x.fsk_band(d.band[0], d.band[1]);
    return;
  }
  const pcm = d?.pcm as Float32Array | undefined;
  if (!pcm || pcm.length === 0) return;
  const ptr = x.fsk_buffer(pcm.length);
  new Float32Array(x.memory.buffer, ptr, pcm.length).set(pcm);
  const n = x.fsk_push(pcm.length);
  if (n > 0) {
    const json = new TextDecoder().decode(new Uint8Array(x.memory.buffer, x.fsk_output(), n));
    post({ run: JSON.parse(json) as FskRun, gen: d.gen });
  }
}

self.onmessage = (e: MessageEvent) => {
  if (fsk) handle(e);
  else if (!failed) queue.push(e);
};

(async () => {
  try {
    const bytes = await (await fetch("fsk.wasm")).arrayBuffer();
    const { instance } = await WebAssembly.instantiate(bytes, {});
    fsk = instance.exports as unknown as Fsk;
    for (const e of queue.splice(0)) handle(e);
  } catch (err) {
    failed = true;
    queue.length = 0;
    post({ error: `RTTY decoder: ${err instanceof Error ? err.message : String(err)}` });
  }
})();
