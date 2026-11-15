// SPDX-License-Identifier: GPL-3.0-or-later
// Web Worker: the PSK31/63/125 skimmer (crates/psk-wasm, psk.wasm) off the
// page's thread. In: {pcm: Float32Array, gen} (12 kHz mono), {band: [lo,
// hi]} (audio Hz) or {reset: true}. Out: {run: PskRun, gen} on new text
// and every 2 s, {error} if it can't load.

import type { PskRun } from "../psk.ts";

interface Psk {
  memory: WebAssembly.Memory;
  psk_buffer(n: number): number;
  psk_push(n: number): number;
  psk_output(): number;
  psk_band(lo: number, hi: number): void;
  psk_reset(): void;
}

const post = (m: unknown) => (self as unknown as Worker).postMessage(m);
const queue: MessageEvent[] = [];
let psk: Psk | null = null;
let failed = false;

function handle(e: MessageEvent): void {
  const x = psk!;
  const d = e.data;
  if (d?.reset) {
    x.psk_reset();
    return;
  }
  if (d?.band) {
    x.psk_band(d.band[0], d.band[1]);
    return;
  }
  const pcm = d?.pcm as Float32Array | undefined;
  if (!pcm || pcm.length === 0) return;
  new Float32Array(x.memory.buffer, x.psk_buffer(pcm.length), pcm.length).set(pcm);
  const n = x.psk_push(pcm.length);
  if (n > 0) {
    const json = new TextDecoder().decode(new Uint8Array(x.memory.buffer, x.psk_output(), n));
    post({ run: JSON.parse(json) as PskRun, gen: d.gen });
  }
}

self.onmessage = (e: MessageEvent) => {
  if (psk) handle(e);
  else if (!failed) queue.push(e);
};

(async () => {
  try {
    const bytes = await (await fetch("psk.wasm")).arrayBuffer();
    const { instance } = await WebAssembly.instantiate(bytes, {});
    psk = instance.exports as unknown as Psk;
    for (const e of queue.splice(0)) handle(e);
  } catch (err) {
    failed = true;
    queue.length = 0;
    post({ error: `PSK decoder didn't load: ${err}` });
  }
})();
