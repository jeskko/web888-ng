// SPDX-License-Identifier: GPL-3.0-or-later
// Signal identification in the page: open URL (tune with its hash, e.g.
// #f=7074.0&m=usb), start audio, pick a passband button, turn ID on, wait,
// print the signals' rows every 15 s; saves sigid.png in OUT.
//   CHROME=... node sigid.mjs URL [PASSBAND_LABEL] [SECONDS]
import { launch, q, sleep } from "./lib.mjs";
const [url, bw = "", secs = "45"] = process.argv.slice(2);
const out = process.env.OUT ?? ".";
const p = await launch({ width: Number(process.env.W ?? 1400), height: Number(process.env.H ?? 900), autoplay: true });
await p.goto(url);
await sleep(3000);
await p.ev(`${q(".start")}?.click()`);
await sleep(1500);
if (bw) await p.ev(`[...document.querySelectorAll('header button')].find((b) => b.textContent === ${JSON.stringify(bw)})?.click()`);
await sleep(500);
const on = await p.ev(`[...document.querySelectorAll('header .tools button')].find((b) => b.textContent === 'ID').classList.contains('on')`);
if (!on) await p.ev(`[...document.querySelectorAll('header .tools button')].find((b) => b.textContent === 'ID').click()`);
const t0 = Date.now();
while (Date.now() - t0 < Number(secs) * 1000) {
  await sleep(15000);
  console.log(`--- ${Math.round((Date.now() - t0) / 1000)} s`);
  console.log(await p.ev(`[...document.querySelectorAll('.sigid tbody tr')].map((r) => [...r.children].map((c) => c.textContent).join(' | ')).join('\\n')`));
}
console.log("note:", await p.ev(`${q(".sigid .spots-list p")}?.textContent ?? ''`));
console.log("freq:", await p.ev(`${q(".freq")}.textContent`), "status:", await p.ev(`${q(".status")}.textContent`));
await p.shot(`${out}/sigid.png`);
p.close();
