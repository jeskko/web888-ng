// SPDX-License-Identifier: GPL-3.0-or-later
// Web Worker: the time-code receiver (crates/timecode-wasm, timecode.wasm)
// off the page's thread. In: {start: [station, tone]} (a new receiver;
// stream time from 0), {pcm: Float32Array, gen} (12 kHz mono). Out: {run:
// TcRun, gen} when a second or a minute is read, {error} if it can't load.

import type { TcRun } from "../timecode.ts";

interface Tc {
  memory: WebAssembly.Memory;
  tc_start(station: number, tone: number): void;
  tc_buffer(n: number): number;
  tc_push(n: number): number;
  tc_output(): number;
}

const post = (m: unknown) => (self as unknown as Worker).postMessage(m);
const queue: MessageEvent[] = [];
let tc: Tc | null = null;
let failed = false;

function handle(e: MessageEvent): void {
  const x = tc!;
  const d = e.data;
  if (d?.start) {
    x.tc_start(d.start[0], d.start[1]);
    return;
  }
  const pcm = d?.pcm as Float32Array | undefined;
  if (!pcm || pcm.length === 0) return;
  // allocate first: it may grow the memory, which detaches the old buffer
  const ptr = x.tc_buffer(pcm.length);
  new Float32Array(x.memory.buffer, ptr, pcm.length).set(pcm);
  const n = x.tc_push(pcm.length);
  if (n > 0) {
    const json = new TextDecoder().decode(new Uint8Array(x.memory.buffer, x.tc_output(), n));
    post({ run: JSON.parse(json) as TcRun, gen: d.gen });
  }
}

self.onmessage = (e: MessageEvent) => {
  if (tc) handle(e);
  else if (!failed) queue.push(e);
};

(async () => {
  try {
    const bytes = await (await fetch("timecode.wasm")).arrayBuffer();
    const { instance } = await WebAssembly.instantiate(bytes, {});
    tc = instance.exports as unknown as Tc;
    for (const e of queue.splice(0)) handle(e);
  } catch (err) {
    failed = true;
    queue.length = 0;
    post({ error: `Time-code receiver didn't load: ${err}` });
  }
})();
