// SPDX-License-Identifier: GPL-3.0-or-later
// Web Worker: the HFDL decoder (crates/hfdl-wasm, hfdl.wasm) off the
// page's thread. In: {pcm: Float32Array, gen} (12 kHz mono, USB),
// {centre: Hz} (the bursts' centre in the audio; a new stream) or {reset:
// true}. Out: {stations} once loaded, {run: HfdlRun, gen} on a burst and
// every 2 s, {error} if it can't load.

import type { HfdlRun, HfdlStation } from "../hfdl.ts";

interface Hfdl {
  memory: WebAssembly.Memory;
  hfdl_buffer(n: number): number;
  hfdl_push(n: number): number;
  hfdl_output(): number;
  hfdl_centre(hz: number): void;
  hfdl_reset(): void;
  hfdl_stations(): number;
}

const post = (m: unknown) => (self as unknown as Worker).postMessage(m);
const queue: MessageEvent[] = [];
let hfdl: Hfdl | null = null;
let failed = false;

const json = (x: Hfdl, n: number) => JSON.parse(new TextDecoder().decode(new Uint8Array(x.memory.buffer, x.hfdl_output(), n)));

function handle(e: MessageEvent): void {
  const x = hfdl!;
  const d = e.data;
  if (d?.reset) {
    x.hfdl_reset();
    return;
  }
  if (typeof d?.centre === "number") {
    x.hfdl_centre(d.centre);
    return;
  }
  const pcm = d?.pcm as Float32Array | undefined;
  if (!pcm || pcm.length === 0) return;
  new Float32Array(x.memory.buffer, x.hfdl_buffer(pcm.length), pcm.length).set(pcm);
  const n = x.hfdl_push(pcm.length);
  if (n > 0) post({ run: json(x, n) as HfdlRun, gen: d.gen });
}

self.onmessage = (e: MessageEvent) => {
  if (hfdl) handle(e);
  else if (!failed) queue.push(e);
};

(async () => {
  try {
    const bytes = await (await fetch("hfdl.wasm")).arrayBuffer();
    const { instance } = await WebAssembly.instantiate(bytes, {});
    hfdl = instance.exports as unknown as Hfdl;
    post({ stations: json(hfdl, hfdl.hfdl_stations()) as HfdlStation[] });
    for (const e of queue.splice(0)) handle(e);
  } catch (err) {
    failed = true;
    queue.length = 0;
    post({ error: `HFDL decoder didn't load: ${err}` });
  }
})();
