// SPDX-License-Identifier: GPL-3.0-or-later
// Web Worker: the signal identifier (crates/sigid-wasm, sigid.wasm) off
// the page's thread. In: {pcm: Float32Array, gen} (12 kHz mono),
// {band: [lo, hi]} (audio Hz), {lsb: boolean}, {reset: true} or
// {analyse: [lo, hi], id, gen} ("what is this?" on audio lo..hi Hz: run
// once 4.09 s of audio is held since the last reset). Out: {run: SigRun,
// gen} after each identification (every 2 s), {an: SigAn | null, id, gen}
// after an analysis, {wait: s, id} while it waits for audio, {error} if
// it can't load.

import type { SigAn } from "../sigdesc.ts";
import type { SigRun } from "../sigid.ts";

interface Sig {
  memory: WebAssembly.Memory;
  sig_buffer(n: number): number;
  sig_push(n: number): number;
  sig_output(): number;
  sig_band(lo: number, hi: number): void;
  sig_lsb(on: number): void;
  sig_reset(): void;
  sig_held(): number;
  sig_analyse(lo: number, hi: number): number;
}

const post = (m: unknown) => (self as unknown as Worker).postMessage(m);
const queue: MessageEvent[] = [];
let sig: Sig | null = null;
let failed = false;
let ask: { lo: number; hi: number; id: number; gen: number } | null = null;
let told = -1;

/** The analysis asked for, once there is enough audio. */
function answer(x: Sig): void {
  if (!ask) return;
  const held = x.sig_held();
  // (it holds 4.096 s at most)
  if (held < 4.09) {
    const left = Math.ceil(4.09 - held);
    if (left !== told) post({ wait: left, id: ask.id });
    told = left;
    return;
  }
  const n = x.sig_analyse(ask.lo, ask.hi);
  const json = new TextDecoder().decode(new Uint8Array(x.memory.buffer, x.sig_output(), n));
  post({ an: (JSON.parse(json) as { an: SigAn | null }).an, id: ask.id, gen: ask.gen });
  ask = null;
  told = -1;
}

function handle(e: MessageEvent): void {
  const x = sig!;
  const d = e.data;
  if (d?.reset) {
    x.sig_reset();
    return;
  }
  if (d?.analyse) {
    ask = { lo: d.analyse[0], hi: d.analyse[1], id: d.id, gen: d.gen };
    told = -1;
    answer(x);
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
  // allocate first: it may grow the memory, which detaches the old buffer
  const ptr = x.sig_buffer(pcm.length);
  new Float32Array(x.memory.buffer, ptr, pcm.length).set(pcm);
  const n = x.sig_push(pcm.length);
  if (n > 0) {
    const json = new TextDecoder().decode(new Uint8Array(x.memory.buffer, x.sig_output(), n));
    post({ run: JSON.parse(json) as SigRun, gen: d.gen });
  }
  answer(x);
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
