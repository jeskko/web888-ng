// SPDX-License-Identifier: GPL-3.0-or-later
// The spot list and the spot labels on the scale: open the list, filter,
// click a row (tunes to the skimmer's dial), zoom in on the labels; saves
// spots-{1,2,3}.png in OUT (default .).
//   CHROME=... node spots.mjs URL   (W, H, DPR: window size, pixel ratio)
import { launch, sleep } from "./lib.mjs";
const url = process.argv[2];
const out = process.env.OUT ?? ".";
const p = await launch({
  width: Number(process.env.W ?? 1400),
  height: Number(process.env.H ?? 900),
  dpr: Number(process.env.DPR ?? 1),
});
await p.goto(url);
await sleep(4000);
const button = "[...document.querySelectorAll('.controls button')].find(b => b.textContent === 'Spots')";
console.log("spots button:", await p.ev(`!!${button}`));
if (!(await p.ev("!!document.querySelector('.spots')"))) await p.ev(`${button}.click()`);
await sleep(1000);
const rows = "document.querySelectorAll('.spots tbody tr')";
console.log("rows:", await p.ev(`${rows}.length`), "count:", await p.ev("document.querySelector('.spots-bar .num').textContent"));
console.log("first:", await p.ev(`${rows}[0]?.textContent`));
await p.shot(`${out}/spots-1.png`);
await p.ev(`(() => { const s = document.querySelector('.spots-bar select'); s.value = '30m'; s.dispatchEvent(new Event('change')); })()`);
await sleep(500);
console.log("30m rows:", await p.ev(`${rows}.length`), "bands:", await p.ev(`[...new Set([...${rows}].map(r => r.title.split(' ')[0]))].join(',')`));
await p.ev(`${rows}[0]?.click()`);
await sleep(2000);
console.log("freq after row click:", await p.ev("document.querySelector('.freq').textContent"), "hash:", await p.ev("location.hash"));
await p.shot(`${out}/spots-2.png`);
// zoom in around the FT8 sub-band so the labels have room
for (let k = 0; k < 4; k++) {
  await p.mouse("mouseWheel", 500, 500, { deltaX: 0, deltaY: -120 });
  await sleep(400);
}
await sleep(3000);
await p.shot(`${out}/spots-3.png`);
p.close();
