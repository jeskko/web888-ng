// SPDX-License-Identifier: GPL-3.0-or-later
// Twin passband tuning: opens the passband popup, moves PBT 1, PBT 2 and
// Shift, and prints the passband the server answers with after each (the
// popup's readout follows the server's state), then Clear.
//   CHROME=... node pbt.mjs URL [shot.png]
import { launch, sleep } from "./lib.mjs";
const [url, shot] = process.argv.slice(2);
const p = await launch({ width: 1512, height: 982, dpr: 2 });
await p.goto(url);
await sleep(5000);
await p.ev(`[...document.querySelectorAll("header button")].find((b) => b.textContent === "USB").click()`); // USB
await sleep(800);
await p.ev(`document.querySelector(".filter-ctl .pop-btn").click()`);
await sleep(400);
const read = () => p.ev(`(() => { const b = document.querySelector(".filter-ctl .pop-btn"); return b.textContent.trim() + " | " + document.querySelector(".filter-ctl .pop-box .pop-row:last-child .num").textContent + (b.classList.contains("on") ? " | PBT on" : ""); })()`);
const slide = async (i, v) => {
  await p.ev(`(() => { const s = document.querySelectorAll(".filter-ctl .pbt-grid input[type=range]")[${i}]; s.value = "${v}"; s.dispatchEvent(new Event("input", { bubbles: true })); })()`);
  await sleep(600);
};
console.log("start:   ", await read());
await slide(0, 400);
console.log("PBT1 +400:", await read());
await slide(1, -500);
console.log("PBT2 -500:", await read());
await slide(2, 150);
console.log("shift +150:", await read());
// a fast sweep (one input every 10 ms): sends are paced, the last one wins
for (let v = 0; v <= 800; v += 20) {
  await p.ev(`(() => { const s = document.querySelectorAll(".filter-ctl .pbt-grid input[type=range]")[0]; s.value = "${v}"; s.dispatchEvent(new Event("input", { bubbles: true })); })()`);
  await sleep(10);
}
await sleep(600);
console.log("sweep to PBT1 +800:", await read());
// a preset keeps PBT on top of it
await p.ev(`[...document.querySelectorAll(".filter-ctl .pop-box .seg button")].find((b) => b.textContent === "3k").click()`);
await sleep(600);
console.log("preset 3k:", await read());
if (shot) await p.shot(shot);
await p.ev(`[...document.querySelectorAll(".filter-ctl .pop-box button")].find((b) => b.textContent === "Clear").click()`);
await sleep(600);
console.log("clear:   ", await read());
// a mode change clears it too
await slide(0, 300);
await p.ev(`[...document.querySelectorAll("header button")].find((b) => b.textContent === "LSB").click()`);
await sleep(800);
console.log("LSB after PBT1 +300:", await read());
p.close();
process.exit(0);
