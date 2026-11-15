// SPDX-License-Identifier: GPL-3.0-or-later
// Web Worker: the JS8 decoder (crates/js8-wasm, js8.wasm) off the page's
// thread. In: {slot: Float32Array, sub: submode id, start, gen} (12 kHz
// mono from the period's start). Out: {result: Js8Result, sub, start,
// gen} per period, {error} if it can't load. Periods are decoded in the
// order they come.

import type { Js8Result } from "../js8.ts";

interface Js8 {
  memory: WebAssembly.Memory;
  js8_buffer(n: number): number;
  js8_decode(submode: number, n: number): number;
  js8_output(): number;
}

const post = (m: unknown) => (self as unknown as Worker).postMessage(m);
const queue: MessageEvent[] = [];
let js8: Js8 | null = null;
let failed = false;

function handle(e: MessageEvent): void {
  const x = js8!;
  const d = e.data;
  const slot = d?.slot as Float32Array | undefined;
  if (!slot || slot.length === 0) return;
  // allocate first: it may grow the memory, which detaches the old buffer
  const ptr = x.js8_buffer(slot.length);
  new Float32Array(x.memory.buffer, ptr, slot.length).set(slot);
  const n = x.js8_decode(d.sub, slot.length);
  const json = new TextDecoder().decode(new Uint8Array(x.memory.buffer, x.js8_output(), n));
  post({ result: JSON.parse(json) as Js8Result, sub: d.sub, start: d.start, gen: d.gen });
}

self.onmessage = (e: MessageEvent) => {
  if (js8) handle(e);
  else if (!failed) queue.push(e);
};

(async () => {
  try {
    const bytes = await (await fetch("js8.wasm")).arrayBuffer();
    const env = { web888_now_ms: () => performance.now() };
    const { instance } = await WebAssembly.instantiate(bytes, { env });
    js8 = instance.exports as unknown as Js8;
    for (const e of queue.splice(0)) handle(e);
  } catch (err) {
    failed = true;
    queue.length = 0;
    post({ error: `JS8 decoder didn't load: ${err}` });
  }
})();
