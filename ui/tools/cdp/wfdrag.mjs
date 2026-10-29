// SPDX-License-Identifier: GPL-3.0-or-later
// The waterfall while it is dragged: a screenshot before, one during the
// drag, one at its end and one after it settles.
//   CHROME=... node wfdrag.mjs URL OUT_PREFIX [seconds] [fraction]
// The drag pulls the view right to left by `fraction` of its width
// (default 0.4) in `seconds` (default 4), in 50 ms steps.
import { launch, sleep } from "./lib.mjs";
const [url, out, secs = "4", frac = "0.4"] = process.argv.slice(2);
const p = await launch({ width: 1400, height: 900 });
await p.goto(url);
await sleep(6000);
await p.shot(`${out}-0-before.png`);
const r = JSON.parse(
  await p.ev("JSON.stringify(document.querySelector('canvas.waterfall').getBoundingClientRect())"),
);
const y = r.y + r.height / 2;
const x0 = r.x + r.width * 0.7;
const steps = Math.round((+secs * 1000) / 50);
const dx = (r.width * +frac) / steps;
await p.mouse("mousePressed", x0, y);
for (let i = 1; i <= steps; i++) {
  await p.mouse("mouseMoved", x0 - i * dx, y);
  await sleep(50);
  if (i === Math.round(steps / 2)) await p.shot(`${out}-1-during.png`);
}
await p.shot(`${out}-2-end.png`);
await p.mouse("mouseReleased", x0 - steps * dx, y);
await sleep(1500);
await p.shot(`${out}-3-after.png`);
console.log(await p.ev("location.hash"));
p.close();
