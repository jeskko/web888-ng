// SPDX-License-Identifier: GPL-3.0-or-later
// PSK31/63/125 decoding in the page: start audio, turn PSK on, pick a band
// from its list (the USB dial, Hz), print the signals (rate, quality,
// text) every 15 s; saves psk.png in OUT.
//   CHROME=... node psk.mjs URL [DIAL_HZ] [SECONDS]
import { launch, q, sleep, decoder } from "./lib.mjs";
const [url, dial = "14070000", secs = "60"] = process.argv.slice(2);
const out = process.env.OUT ?? ".";
const p = await launch({ width: Number(process.env.W ?? 1400), height: Number(process.env.H ?? 900), autoplay: true });
await p.goto(url);
await sleep(3000);
await p.ev(`${q(".start")}?.click()`);
await sleep(1500);
await decoder(p, "PSK");
await sleep(500);
await p.ev(`(() => { const s = ${q(".psk select")}; s.value = ${JSON.stringify(dial)}; s.dispatchEvent(new Event('change', { bubbles: true })); })()`);
const t0 = Date.now();
while (Date.now() - t0 < Number(secs) * 1000) {
  await sleep(15000);
  console.log(`--- ${Math.round((Date.now() - t0) / 1000)} s`);
  console.log(await p.ev(`[...document.querySelectorAll('.psk .rtty-sig')].map((d) => d.querySelector('.rtty-head').textContent + '\\n  ' + (d.querySelector('.rtty-text')?.textContent ?? '').slice(-300).replace(/\\n/g, '\\n  ')).join('\\n')`));
}
console.log("freq:", await p.ev(`${q(".freq")}.textContent`), "mode:", await p.ev(`${q("header button.on")}?.textContent`));
await p.shot(`${out}/psk.png`);
p.close();
