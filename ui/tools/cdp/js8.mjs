// SPDX-License-Identifier: GPL-3.0-or-later
// JS8 decoding in the page: start audio, turn JS8 on, pick a band from
// its list (the USB dial, Hz), print the messages every 30 s; saves
// js8.png in OUT (with MAP=1, the map too: js8-map.png).
//   CHROME=... node js8.mjs URL [DIAL_HZ] [SECONDS]
import { launch, q, sleep, decoder, mapShot, lit } from "./lib.mjs";
const [url, dial = "7078000", secs = "120"] = process.argv.slice(2);
const out = process.env.OUT ?? ".";
const p = await launch({ width: Number(process.env.W ?? 1400), height: Number(process.env.H ?? 900), autoplay: true });
await p.goto(url);
await sleep(3000);
await p.ev(`${q(".start")}?.click()`);
await sleep(1500);
await decoder(p, "JS8");
await sleep(500);
await p.ev(`(() => { const s = ${q(".js8 select")}; s.value = ${lit(dial)}; s.dispatchEvent(new Event('change', { bubbles: true })); })()`);
const t0 = Date.now();
while (Date.now() - t0 < Number(secs) * 1000) {
  await sleep(30000);
  console.log(`--- ${Math.round((Date.now() - t0) / 1000)} s, frames ${await p.ev(`${q(".js8 .spots-bar .num")}?.textContent`)} (${await p.ev(`${q(".js8 .spots-bar .num")}?.title`)})`);
  console.log(await p.ev(`[...document.querySelectorAll('.js8 .js8-msg')].slice(0, 10).map((d) => d.innerText.replace(/\\n/g, '  ')).join('\\n')`));
}
console.log("heard:", await p.ev(`${q(".js8 .js8-heard")}?.innerText`));
console.log("freq:", await p.ev(`${q(".freq")}.textContent`), "mode:", await p.ev(`${q("header button.on")}?.textContent`));
await p.shot(`${out}/js8.png`);
if (process.env.MAP) await mapShot(p, `${out}/js8-map.png`);
p.close();
