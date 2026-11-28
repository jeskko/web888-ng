// SPDX-License-Identifier: GPL-3.0-or-later
// DGPS beacon decoding in the page: start audio, turn DGPS on, pick a part
// of the beacon band from its list (the USB dial, Hz), print the beacons
// (station, health, position, satellites, messages) every 15 s; saves
// dgps.png in OUT.
//   CHROME=... node dgps.mjs URL [DIAL_HZ] [SECONDS]
import { launch, q, sleep, decoder, lit } from "./lib.mjs";
const [url, dial = "286500", secs = "60"] = process.argv.slice(2);
const out = process.env.OUT ?? ".";
const p = await launch({ width: Number(process.env.W ?? 1400), height: Number(process.env.H ?? 900), autoplay: true });
await p.goto(url);
await sleep(3000);
await p.ev(`${q(".start")}?.click()`);
await sleep(1500);
await decoder(p, "DGPS");
await sleep(500);
await p.ev(`(() => { const s = ${q(".dgps select")}; const o = [...s.options].find((o) => o.value === ${lit(dial)}); if (o) { s.value = o.value; } else { s.value = s.options[1].value; } s.dispatchEvent(new Event('change', { bubbles: true })); })()`);
const t0 = Date.now();
while (Date.now() - t0 < Number(secs) * 1000) {
  await sleep(15000);
  console.log(`--- ${Math.round((Date.now() - t0) / 1000)} s`);
  console.log(await p.ev(`[...document.querySelectorAll('.dgps .rtty-sig')].map((d) => [...d.children].map((c) => c.textContent).join('\\n  ')).join('\\n') || document.querySelector('.dgps .spots-list')?.textContent`));
}
console.log("freq:", await p.ev(`${q(".freq")}.textContent`));
await p.shot(`${out}/dgps.png`);
p.close();
