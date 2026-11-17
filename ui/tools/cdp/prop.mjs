// SPDX-License-Identifier: GPL-3.0-or-later
// The propagation panel: all regions now, one region's day, a mode
// change, a band's tune; with W/H a phone-sized page.
//   CHROME=... OUT=/tmp/x [W=390 H=844 DPR=3] node prop.mjs URL
import { resolve } from "node:path";
import { launch, q, sleep } from "./lib.mjs";
const [url] = process.argv.slice(2);
const out = resolve(process.env.OUT ?? ".");
const W = Number(process.env.W ?? 1400);
const p = await launch({ width: W, height: Number(process.env.H ?? 900), dpr: Number(process.env.DPR ?? 1) });
await p.goto(new URL("#propagation", url).href);
await sleep(3000);
console.log("title:", await p.ev(`${q(".prop-what")}?.textContent`));
console.log("rows:", await p.ev(`document.querySelectorAll(".prop-now tbody tr").length`), "| cells:", await p.ev(`document.querySelectorAll(".prop-now .prop-cell").length`));
console.log("basis:", await p.ev(`${q(".prop-basis")}?.textContent`));
await p.shot(`${out}/prop-now-${W}.png`);
await p.ev(`[...document.querySelectorAll(".prop-opts .seg button")].find((b) => b.textContent === "FT8").click()`);
await sleep(300);
console.log("FT8 Europe:", await p.ev(`[...document.querySelectorAll(".prop-now tbody tr")][0].textContent`));
await p.ev(`${q(".prop-link")}.click()`);
await sleep(300);
console.log("day rows:", await p.ev(`document.querySelectorAll(".prop-day tbody tr").length`), "| hour cells:", await p.ev(`document.querySelectorAll(".prop-day .prop-hour").length`));
await p.shot(`${out}/prop-day-${W}.png`);
console.log("page scrolls sideways:", await p.ev(`document.documentElement.scrollWidth > innerWidth`));
p.close();
