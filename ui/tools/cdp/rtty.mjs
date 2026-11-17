// SPDX-License-Identifier: GPL-3.0-or-later
// FSK/RTTY decoding in the page: start audio, turn RTTY on, pick a station
// from its list (the centre between the tones, Hz; "-" keeps the URL's
// tuning), print the signals
// (parameters and text) every 15 s; saves rtty.png in OUT.
//   CHROME=... [WIDE=0|1] node rtty.mjs URL [STATION_HZ] [SECONDS]
import { launch, q, sleep, decoder, lit } from "./lib.mjs";
const [url, station = "10100800", secs = "60"] = process.argv.slice(2);
const out = process.env.OUT ?? ".";
const p = await launch({ width: Number(process.env.W ?? 1400), height: Number(process.env.H ?? 900), autoplay: true });
await p.goto(url);
await sleep(3000);
await p.ev(`${q(".start")}?.click()`);
await sleep(1500);
await decoder(p, "RTTY");
await sleep(500);
// "-": keep the tuning (the URL's hash, e.g. #f=7091.0&m=usb)
if (station !== "-") await p.ev(`(() => { const s = ${q(".rtty select")}; s.value = ${lit(station)}; s.dispatchEvent(new Event('change', { bubbles: true })); })()`);
// WIDE=1: skim the channelizer bin around the dial (needs bins mode)
if (process.env.WIDE !== undefined) {
  await sleep(500);
  const on = await p.ev(`[...document.querySelectorAll('.rtty .spots-bar button')].find((b) => b.textContent === 'Wide')?.classList.contains('on')`);
  if (on !== (process.env.WIDE === "1")) await p.ev(`[...document.querySelectorAll('.rtty .spots-bar button')].find((b) => b.textContent === 'Wide')?.click()`);
}
const t0 = Date.now();
while (Date.now() - t0 < Number(secs) * 1000) {
  await sleep(15000);
  console.log(`--- ${Math.round((Date.now() - t0) / 1000)} s`);
  console.log(await p.ev(`[...document.querySelectorAll('.rtty-sig')].map((d) => d.querySelector('.rtty-head').textContent + '\\n  ' + (d.querySelector('.rtty-text')?.textContent ?? '').slice(-300).replace(/\\n/g, '\\n  ')).join('\\n')`));
}
console.log("freq:", await p.ev(`${q(".freq")}.textContent`), "mode:", await p.ev(`${q("header button.on")}?.textContent`));
await p.shot(`${out}/rtty.png`);
p.close();
