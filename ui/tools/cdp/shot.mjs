// SPDX-License-Identifier: GPL-3.0-or-later
// One screenshot at a given size and pixel ratio (layout checks), plus
// the canvases' sizes.
//   CHROME=... node shot.mjs URL OUT.png [width height dpr]
import { launch, sleep } from "./lib.mjs";
const [url, out, w = "1400", h = "900", dpr = "1"] = process.argv.slice(2);
const p = await launch({ width: +w, height: +h, dpr: +dpr });
await p.cdp("Network.enable");
await p.cdp("Network.setCacheDisabled", { cacheDisabled: true });
await p.goto(url);
await sleep(8000);
console.log(
  await p.ev(
    "JSON.stringify([...document.querySelectorAll('canvas')].map(c => [c.className, c.width, c.height, c.clientWidth, c.clientHeight]))",
  ),
);
console.log("horizontal scroll:", await p.ev("document.documentElement.scrollWidth > window.innerWidth"));
await p.shot(out);
p.close();
