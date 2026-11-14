// SPDX-License-Identifier: GPL-3.0-or-later
// Web Worker: the fax and SSTV decoders (crates/img-wasm, img.wasm) off the
// page's thread. In: {pcm: Float32Array, gen} (12 kHz mono), {reset: true},
// {cmd: "faxStart" | "faxStop" | "sstvStop"}, {cmd: "faxAdjust", ppm,
// shift}, {cmd: "faxIoc", ioc}. Out: {run: ImgRun, blob: Uint8Array, gen}
// on events and about once a second, {error} if it can't load.

import type { ImgRun } from "../img.ts";

interface Img {
  memory: WebAssembly.Memory;
  img_buffer(n: number): number;
  img_push(n: number): number;
  img_output(): number;
  img_blob(): number;
  img_blob_len(): number;
  img_fax_start(): void;
  img_fax_stop(): void;
  img_fax_adjust(ppm: number, shift: number): void;
  img_fax_ioc(ioc: number): void;
  img_sstv_stop(): void;
  img_reset(): void;
}

const post = (m: unknown, t?: Transferable[]) => (self as unknown as Worker).postMessage(m, t ?? []);
const queue: MessageEvent[] = [];
let img: Img | null = null;
let failed = false;

function handle(e: MessageEvent): void {
  const x = img!;
  const d = e.data;
  if (d?.reset) {
    x.img_reset();
    return;
  }
  switch (d?.cmd) {
    case "faxStart":
      x.img_fax_start();
      return;
    case "faxStop":
      x.img_fax_stop();
      return;
    case "faxAdjust":
      x.img_fax_adjust(d.ppm, d.shift);
      return;
    case "faxIoc":
      x.img_fax_ioc(d.ioc);
      return;
    case "sstvStop":
      x.img_sstv_stop();
      return;
  }
  const pcm = d?.pcm as Float32Array | undefined;
  if (!pcm || pcm.length === 0) return;
  const ptr = x.img_buffer(pcm.length);
  new Float32Array(x.memory.buffer, ptr, pcm.length).set(pcm);
  const n = x.img_push(pcm.length);
  if (n > 0) {
    const json = new TextDecoder().decode(new Uint8Array(x.memory.buffer, x.img_output(), n));
    const blob = new Uint8Array(x.memory.buffer, x.img_blob(), x.img_blob_len()).slice();
    post({ run: JSON.parse(json) as ImgRun, blob, gen: d.gen }, [blob.buffer]);
  }
}

self.onmessage = (e: MessageEvent) => {
  if (img) handle(e);
  else if (!failed) queue.push(e);
};

(async () => {
  try {
    const bytes = await (await fetch("img.wasm")).arrayBuffer();
    const { instance } = await WebAssembly.instantiate(bytes, {});
    img = instance.exports as unknown as Img;
    for (const e of queue.splice(0)) handle(e);
  } catch (err) {
    failed = true;
    queue.length = 0;
    post({ error: `FAX/SSTV decoder: ${err instanceof Error ? err.message : String(err)}` });
  }
})();
