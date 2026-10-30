// SPDX-License-Identifier: GPL-3.0-or-later
// Web Worker: the JTTY receiver (crates/jtty-wasm, jtty.wasm) off the page's
// thread. In: {pcm: Float32Array, gen} (12 kHz mono) or {reset: true}.
// Out: {decodes: JttyDecode[], gen} as messages complete (`gen` of the
// samples that completed them), {error} if it can't load.

import type { JttyDecode } from "../jtty.ts";

interface Jtty {
  memory: WebAssembly.Memory;
  jtty_buffer(n: number): number;
  jtty_push(n: number): number;
  jtty_output(): number;
  jtty_reset(): void;
}

const post = (m: unknown) => (self as unknown as Worker).postMessage(m);
const queue: MessageEvent[] = [];
let jtty: Jtty | null = null;
let failed = false;

function handle(e: MessageEvent): void {
  const x = jtty!;
  const d = e.data;
  if (d?.reset) {
    x.jtty_reset();
    return;
  }
  const pcm = d?.pcm as Float32Array | undefined;
  if (!pcm || pcm.length === 0) return;
  const ptr = x.jtty_buffer(pcm.length);
  new Float32Array(x.memory.buffer, ptr, pcm.length).set(pcm);
  const n = x.jtty_push(pcm.length);
  if (n > 2) {
    const json = new TextDecoder().decode(new Uint8Array(x.memory.buffer, x.jtty_output(), n));
    post({ decodes: JSON.parse(json) as JttyDecode[], gen: d.gen });
  }
}

self.onmessage = (e: MessageEvent) => {
  if (jtty) handle(e);
  else if (!failed) queue.push(e);
};

(async () => {
  try {
    const bytes = await (await fetch("jtty.wasm")).arrayBuffer();
    const { instance } = await WebAssembly.instantiate(bytes, {});
    jtty = instance.exports as unknown as Jtty;
    for (const e of queue.splice(0)) handle(e);
  } catch (err) {
    failed = true;
    queue.length = 0;
    post({ error: `JTTY decoder: ${err instanceof Error ? err.message : String(err)}` });
  }
})();
