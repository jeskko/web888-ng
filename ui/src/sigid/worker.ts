// SPDX-License-Identifier: GPL-3.0-or-later
// Web Worker: the signal identifier (crates/sigid-wasm, sigid.wasm) off
// the page's thread. In: {pcm: Float32Array, gen} (12 kHz mono),
// {band: [lo, hi]} (audio Hz), {lsb: boolean} or {reset: true}. Out:
// {run: SigRun, gen} after each identification (every 2 s), {error} if it
// can't load.

import type { SigRun } from "../sigid.ts";

interface Sig {
  memory: WebAssembly.Memory;
  sig_buffer(n: number): number;
  sig_push(n: number): number;
  sig_output(): number;
  sig_band(lo: number, hi: number): void;
  sig_lsb(on: number): void;
  sig_reset(): void;
}

const post = (m: unknown) => (self as unknown as Worker).postMessage(m);
const queue: MessageEvent[] = [];
let sig: Sig | null = null;
let failed = false;

function handle(e: MessageEvent): void {
  const x = sig!;
  const d = e.data;
  if (d?.reset) {
    x.sig_reset();
    return;
  }
  if (d?.band) {
    x.sig_band(d.band[0], d.band[1]);
    return;
  }
  if (typeof d?.lsb === "boolean") {
    x.sig_lsb(d.lsb ? 1 : 0);
    return;
  }
  const pcm = d?.pcm as Float32Array | undefined;
  if (!pcm || pcm.length === 0) return;
  new Float32Array(x.memory.buffer, x.sig_buffer(pcm.length), pcm.length).set(pcm);
  const n = x.sig_push(pcm.length);
  if (n > 0) {
    const json = new TextDecoder().decode(new Uint8Array(x.memory.buffer, x.sig_output(), n));
    post({ run: JSON.parse(json) as SigRun, gen: d.gen });
  }
}

self.onmessage = (e: MessageEvent) => {
  if (sig) handle(e);
  else if (!failed) queue.push(e);
};

(async () => {
  try {
    const bytes = await (await fetch("sigid.wasm")).arrayBuffer();
    const { instance } = await WebAssembly.instantiate(bytes, {});
    sig = instance.exports as unknown as Sig;
    for (const e of queue.splice(0)) handle(e);
  } catch (err) {
    failed = true;
    queue.length = 0;
    post({ error: `Signal identifier didn't load: ${err}` });
  }
})();
