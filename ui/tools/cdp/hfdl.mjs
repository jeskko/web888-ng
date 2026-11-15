// SPDX-License-Identifier: GPL-3.0-or-later
// HFDL decoding in the page: start audio, turn HFDL on, pick a channel
// from its list (the listed frequency, Hz), print the decoded bursts every
// 15 s; saves hfdl.png in OUT (with MAP=1, the map too: hfdl-map.png).
//   CHROME=... node hfdl.mjs URL [DIAL_HZ] [SECONDS]
import { launch, q, sleep, decoder, mapShot } from "./lib.mjs";
const [url, dial = "8942000", secs = "60"] = process.argv.slice(2);
const out = process.env.OUT ?? ".";
const p = await launch({ width: Number(process.env.W ?? 1400), height: Number(process.env.H ?? 900), autoplay: true });
await p.goto(url);
await sleep(3000);
await p.ev(`${q(".start")}?.click()`);
await sleep(1500);
await decoder(p, "HFDL");
// the channel list arrives with the worker
await sleep(2500);
await p.ev(`(() => { const s = ${q(".hfdl select")}; s.value = ${JSON.stringify(dial)}; s.dispatchEvent(new Event('change', { bubbles: true })); })()`);
const t0 = Date.now();
while (Date.now() - t0 < Number(secs) * 1000) {
  await sleep(15000);
  console.log(`--- ${Math.round((Date.now() - t0) / 1000)} s, decoded/heard ${await p.ev(`${q(".hfdl .spots-bar .num")}?.textContent`)}`);
  console.log(await p.ev(`[...document.querySelectorAll('.hfdl .hfdl-msg')].slice(0, 8).map((d) => d.innerText.replace(/\\n/g, '\\n  ')).join('\\n')`));
}
console.log("freq:", await p.ev(`${q(".freq")}.textContent`), "mode:", await p.ev(`${q("header button.on")}?.textContent`));
await p.shot(`${out}/hfdl.png`);
if (process.env.MAP) await mapShot(p, `${out}/hfdl-map.png`);
p.close();
