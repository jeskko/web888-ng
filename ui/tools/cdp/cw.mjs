// SPDX-License-Identifier: GPL-3.0-or-later
// CW decoding in the page: open URL (tune with its hash, e.g.
// #f=7018.0&m=usb), start audio, pick a passband button, turn CW on, wait,
// print the signals' rows; saves cw.png in OUT.
//   CHROME=... [NEURAL=0|1] [WIDE=0|1] node cw.mjs URL [PASSBAND_LABEL] [SECONDS]
import { launch, q, sleep, decoder, lit } from "./lib.mjs";
const [url, bw = "", secs = "60"] = process.argv.slice(2);
const out = process.env.OUT ?? ".";
const p = await launch({ width: Number(process.env.W ?? 1400), height: Number(process.env.H ?? 900), autoplay: true });
await p.goto(url);
await sleep(3000);
await p.ev(`${q(".start")}?.click()`);
await sleep(1500);
// the passband is a drop-down: open it, pick
if (bw) {
  await p.ev(`document.querySelector('.drop-bw .drop-btn')?.click()`);
  await sleep(100);
  await p.ev(`[...document.querySelectorAll('.drop-bw .drop-pop button')].find((b) => b.textContent === ${lit(bw)})?.click()`);
}
await sleep(500);
await decoder(p, "CW");
// WIDE=1: skim the channelizer bin around the dial (needs bins mode)
if (process.env.WIDE !== undefined) {
  await sleep(300);
  const on = await p.ev(`[...document.querySelectorAll('.cw .spots-bar button')].find((b) => b.textContent === 'Wide')?.classList.contains('on')`);
  if (on !== (process.env.WIDE === "1")) await p.ev(`[...document.querySelectorAll('.cw .spots-bar button')].find((b) => b.textContent === 'Wide')?.click()`);
}
// NEURAL=1: the neural decoder's text (the button toggles the remembered setting)
if (process.env.NEURAL !== undefined) {
  await sleep(300);
  const on = await p.ev(`[...document.querySelectorAll('.cw .spots-bar button')].find((b) => b.textContent === 'Neural').classList.contains('on')`);
  if (on !== (process.env.NEURAL === "1")) await p.ev(`[...document.querySelectorAll('.cw .spots-bar button')].find((b) => b.textContent === 'Neural').click()`);
}
const t0 = Date.now();
while (Date.now() - t0 < Number(secs) * 1000) {
  await sleep(15000);
  console.log(`--- ${Math.round((Date.now() - t0) / 1000)} s`);
  console.log(await p.ev(`[...document.querySelectorAll('.cw tbody tr')].map((r) => [...r.children].map((c) => c.textContent).join(' | ')).join('\\n')`));
}
console.log("note:", await p.ev(`${q(".cw .spots-list p")}?.textContent ?? ''`));
console.log("freq:", await p.ev(`${q(".freq")}.textContent`), "status:", await p.ev(`${q(".status")}.textContent`));
console.log("panel:", await p.ev(`${q(".cw .spots-list")}.textContent.slice(0, 200)`));
await p.shot(`${out}/cw.png`);
p.close();
