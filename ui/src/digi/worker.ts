// SPDX-License-Identifier: GPL-3.0-or-later
// Web Worker: the skimmers' FT8, FT4 and WSPR decoders (crates/digi-wasm,
// digi.wasm) off the page's thread. In: {slot: Float32Array, mode, deep,
// budget, start, gen} (12 kHz mono from the slot's start) or {reset: true}
// (another band: forget the callsigns heard). Out: {result: DigiResult,
// start, gen} per slot, {error} if it can't load. Slots are decoded in
// the order they come.

import type { DigiResult } from "../digi.ts";

interface Digi {
  memory: WebAssembly.Memory;
  digi_buffer(n: number): number;
  digi_decode(mode: number, deep: number, n: number, budgetMs: number): number;
  digi_output(): number;
  digi_reset(): void;
}

const post = (m: unknown) => (self as unknown as Worker).postMessage(m);
const queue: MessageEvent[] = [];
let digi: Digi | null = null;
let failed = false;

function handle(e: MessageEvent): void {
  const x = digi!;
  const d = e.data;
  if (d?.reset) {
    x.digi_reset();
    return;
  }
  const slot = d?.slot as Float32Array | undefined;
  if (!slot || slot.length === 0) return;
  const ptr = x.digi_buffer(slot.length);
  new Float32Array(x.memory.buffer, ptr, slot.length).set(slot);
  const n = x.digi_decode(d.mode, d.deep ? 1 : 0, slot.length, d.budget ?? 0);
  const json = new TextDecoder().decode(new Uint8Array(x.memory.buffer, x.digi_output(), n));
  post({ result: JSON.parse(json) as DigiResult, start: d.start, gen: d.gen });
}

self.onmessage = (e: MessageEvent) => {
  if (digi) handle(e);
  else if (!failed) queue.push(e);
};

(async () => {
  try {
    const bytes = await (await fetch("digi.wasm")).arrayBuffer();
    // the decoders' deadlines and timing (web888-clock)
    const env = { web888_now_ms: () => performance.now() };
    const { instance } = await WebAssembly.instantiate(bytes, { env });
    digi = instance.exports as unknown as Digi;
    for (const e of queue.splice(0)) handle(e);
  } catch (err) {
    failed = true;
    queue.length = 0;
    post({ error: `Decoder: ${err instanceof Error ? err.message : String(err)}` });
  }
})();
