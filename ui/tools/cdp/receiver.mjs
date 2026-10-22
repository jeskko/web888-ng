// SPDX-License-Identifier: GPL-3.0-or-later
// The receiver page: start audio, zoom the waterfall, click-tune, switch
// mode; prints readouts and saves receiver-{1,2,3}.png in OUT (default .).
//   CHROME=... node receiver.mjs URL   (W, H, DPR: window size, pixel ratio)
import { launch, sleep } from "./lib.mjs";
const url = process.argv[2];
const out = process.env.OUT ?? ".";
const p = await launch({
  width: Number(process.env.W ?? 1400),
  height: Number(process.env.H ?? 900),
  dpr: Number(process.env.DPR ?? 1),
  autoplay: true,
});
await p.goto(url);
await sleep(3000);
console.log("secure:", await p.ev("window.isSecureContext"));
await p.ev("document.querySelector('.start')?.click()");
await sleep(6000);
console.log("freq:", await p.ev("document.querySelector('.freq').textContent"));
console.log("status:", await p.ev("document.querySelector('.status').textContent"));
console.log("smeter:", await p.ev("document.querySelector('.smeter').textContent"));
await p.shot(`${out}/receiver-1.png`);
// zoom in three steps around x = 420, on the waterfall
for (let k = 0; k < 3; k++) {
  await p.mouse("mouseWheel", 420, 500, { deltaX: 0, deltaY: -120 });
  await sleep(400);
}
await sleep(4000);
console.log("span:", await p.ev("document.querySelectorAll('.controls .num')[1].textContent"));
await p.shot(`${out}/receiver-2.png`);
await p.mouse("mousePressed", 700, 450);
await p.mouse("mouseReleased", 700, 450);
await sleep(2000);
console.log("freq after click:", await p.ev("document.querySelector('.freq').textContent"), "hash:", await p.ev("location.hash"));
await p.ev("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'l'}))");
await sleep(3500);
console.log("mode:", await p.ev("document.querySelector('header button.on').textContent"));
console.log("status:", await p.ev("document.querySelector('.status').textContent"));
await p.shot(`${out}/receiver-3.png`);
p.close();
