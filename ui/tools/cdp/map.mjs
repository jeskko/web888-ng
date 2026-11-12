// SPDX-License-Identifier: GPL-3.0-or-later
// The map: opens it, counts the stations shown, takes screenshots in the
// three projections, drags the great circle map and zooms, and hovers a station.
//   CHROME=... node map.mjs URL OUT_PREFIX
import { launch, q, sleep } from "./lib.mjs";
const [url, out = "/tmp/map"] = process.argv.slice(2);
const p = await launch({ width: 1600, height: 900 });
await p.goto(url);
await sleep(5000);
// open it (the browser profile may remember it open)
await p.ev(`document.querySelector("aside.map") || [...document.querySelectorAll("header .tools button")].find((b) => b.textContent.trim() === "Map").click()`);
await sleep(1000);
const setProj = (v) =>
  p.ev(`(() => { const s = ${q(".map .spots-bar select")}; s.value = "${v}"; s.dispatchEvent(new Event("change", { bubbles: true })); })()`);
await setProj("azim");
await sleep(5000);
const count = async () => p.ev(`${q(".map .spots-bar .num")}?.textContent`);
console.log("stations shown (great circle):", await count());
console.log("info", await p.ev(`fetch("api/map/info").then((r) => r.json())`));
console.log("dx with places", await p.ev(`fetch("api/map/dx").then((r) => r.json()).then((j) => j.spots.filter((s) => s.dx_pos).length + " of " + j.spots.length)`));
await p.shot(`${out}-azim.png`);
const r = JSON.parse(await p.ev(`JSON.stringify(${q(".map-box canvas")}.getBoundingClientRect())`));
const cx = r.x + r.width / 2;
const cy = r.y + r.height / 2;
// drag: turn the globe under the centre
await p.mouse("mousePressed", cx, cy);
for (let i = 1; i <= 20; i++) {
  await p.mouse("mouseMoved", cx - i * 10, cy + i * 4);
  await sleep(30);
}
await p.mouse("mouseReleased", cx - 200, cy + 80);
await sleep(800);
await p.shot(`${out}-azim-turned.png`);
// zoom in on the middle
for (let i = 0; i < 6; i++) {
  await p.cdp("Input.dispatchMouseEvent", { type: "mouseWheel", x: cx, y: cy, deltaX: 0, deltaY: -200 });
  await sleep(100);
}
await sleep(800);
await p.shot(`${out}-azim-zoom.png`);
await setProj("globe");
await sleep(1500);
console.log("stations shown (globe):", await count());
await p.shot(`${out}-globe.png`);
await setProj("merc");
await sleep(300);
await p.ev(`[...document.querySelectorAll(".map .spots-bar button")].find((b) => b.textContent.includes("⌖")).click()`);
await sleep(1500);
console.log("stations shown (Mercator):", await count());
await p.shot(`${out}-merc.png`);
// hover: sweep across the map until a tooltip shows
let tip = null;
for (let y = r.y + 40; y < r.y + r.height - 20 && !tip; y += 12) {
  for (let x = r.x + 20; x < r.x + r.width - 20 && !tip; x += 12) {
    await p.mouse("mouseMoved", x, y, { button: "none" });
    tip = await p.ev(`${q(".map-tip")}?.innerText ?? null`);
  }
}
console.log("tooltip:", tip);
await p.shot(`${out}-hover.png`);
p.close();
