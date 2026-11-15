// SPDX-License-Identifier: GPL-3.0-or-later
// Fax/SSTV decoding in the page: start audio, turn FAX/SSTV on, pick a
// station from its list ("f<carrier Hz>" fax, "s<dial Hz>" SSTV), optionally
// start a fax by hand (no start tone heard), wait, print the status line
// and save img.png in OUT.
//   CHROME=... node img.mjs URL [STATION] [SECONDS] [start]
import { launch, q, sleep, decoder } from "./lib.mjs";
const [url, station = "f7880000", secs = "60", start = ""] = process.argv.slice(2);
const out = process.env.OUT ?? ".";
const p = await launch({ width: Number(process.env.W ?? 1400), height: Number(process.env.H ?? 900), autoplay: true });
await p.goto(url);
await sleep(3000);
await p.ev(`${q(".start")}?.click()`);
await sleep(1500);
await decoder(p, "FAX/SSTV");
await sleep(500);
await p.ev(`(() => { const s = ${q(".img select")}; s.value = ${JSON.stringify(station)}; s.dispatchEvent(new Event('change', { bubbles: true })); })()`);
await sleep(3000);
if (start) await p.ev(`[...document.querySelectorAll('.img-ctl button')].find((b) => b.textContent === 'Fax start').click()`);
const t0 = Date.now();
while (Date.now() - t0 < Number(secs) * 1000) {
  await sleep(15000);
  console.log(`${Math.round((Date.now() - t0) / 1000)} s:`, await p.ev(`${q(".img-status")}.textContent`), "|", await p.ev(`${q(".img-head")}?.textContent ?? ''`));
}
console.log("freq:", await p.ev(`${q(".freq")}.textContent`));
await p.shot(`${out}/img.png`);
p.close();
