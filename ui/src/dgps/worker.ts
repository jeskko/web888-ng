// SPDX-License-Identifier: GPL-3.0-or-later
// Web Worker: the DGPS beacon skimmer (crates/dgps-wasm, dgps.wasm) off the
// page's thread. In: {pcm: Float32Array, gen} (12 kHz mono), {band: [lo,
// hi]} (audio Hz), {wide: {rate, lo, hi} | null} (IQ from a channelizer
// bin, or back to audio), {iq: Float32Array (I, Q interleaved), gen} or
// {reset: true}. Out: {run: DgpsRun, gen} on new messages
// and every 2 s, {error} if it can't load.

import type { DgpsRun } from "../dgps.ts";

interface Dgps {
  memory: WebAssembly.Memory;
  dgps_buffer(n: number): number;
  dgps_push(n: number): number;
  dgps_output(): number;
  dgps_band(lo: number, hi: number): void;
  dgps_reset(): void;
  dgps_wide(rate: number, lo: number, hi: number): void;
  dgps_buffer_iq(n: number): number;
  dgps_push_iq(n: number): number;
}

const post = (m: unknown) => (self as unknown as Worker).postMessage(m);
const queue: MessageEvent[] = [];
let dgps: Dgps | null = null;
let failed = false;

function handle(e: MessageEvent): void {
  const x = dgps!;
  const d = e.data;
  if (d?.reset) {
    x.dgps_reset();
    return;
  }
  if (d && "wide" in d) {
    if (d.wide) x.dgps_wide(d.wide.rate, d.wide.lo, d.wide.hi);
    else x.dgps_wide(0, 0, 0);
    return;
  }
  if (d?.iq) {
    const iq = d.iq as Float32Array;
    const n = iq.length >> 1;
    const p = x.dgps_buffer_iq(n);
    new Float32Array(x.memory.buffer, p, 2 * n).set(iq);
    const m = x.dgps_push_iq(n);
    if (m > 0) {
      const json = new TextDecoder().decode(new Uint8Array(x.memory.buffer, x.dgps_output(), m));
      post({ run: JSON.parse(json) as DgpsRun, gen: d.gen });
    }
    return;
  }
  if (d?.band) {
    x.dgps_band(d.band[0], d.band[1]);
    return;
  }
  const pcm = d?.pcm as Float32Array | undefined;
  if (!pcm || pcm.length === 0) return;
  // allocate first: it may grow the memory, which detaches the old buffer
  const ptr = x.dgps_buffer(pcm.length);
  new Float32Array(x.memory.buffer, ptr, pcm.length).set(pcm);
  const n = x.dgps_push(pcm.length);
  if (n > 0) {
    const json = new TextDecoder().decode(new Uint8Array(x.memory.buffer, x.dgps_output(), n));
    post({ run: JSON.parse(json) as DgpsRun, gen: d.gen });
  }
}

self.onmessage = (e: MessageEvent) => {
  if (dgps) handle(e);
  else if (!failed) queue.push(e);
};

(async () => {
  try {
    const bytes = await (await fetch("dgps.wasm")).arrayBuffer();
    const { instance } = await WebAssembly.instantiate(bytes, {});
    dgps = instance.exports as unknown as Dgps;
    for (const e of queue.splice(0)) handle(e);
  } catch (err) {
    failed = true;
    queue.length = 0;
    post({ error: `DGPS decoder didn't load: ${err}` });
  }
})();
