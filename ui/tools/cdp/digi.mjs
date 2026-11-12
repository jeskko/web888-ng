// SPDX-License-Identifier: GPL-3.0-or-later
// FT8/FT4/WSPR decoding in the page: start audio, open the panel, pick the
// mode and a dial from its list, wait, print the status line and the
// decoded rows; saves digi.png in OUT.
//   CHROME=... node digi.mjs URL [MODE] [DIAL_HZ] [SECONDS] [DEEP]
import { launch, q, sleep } from "./lib.mjs";
const [url, mode = "ft8", dial = "7074000", secs = "50", deep = ""] = process.argv.slice(2);
const out = process.env.OUT ?? ".";
const p = await launch({ width: Number(process.env.W ?? 1400), height: Number(process.env.H ?? 900), autoplay: true });
await p.goto(url);
await sleep(3000);
await p.ev(`${q(".start")}?.click()`);
await sleep(2000);
await p.ev(`[...document.querySelectorAll('header .tools button')].find((b) => b.textContent === 'FT8/FT4/WSPR').click()`);
await sleep(500);
const pick = (sel, v) => p.ev(`(() => { const s = document.querySelectorAll('.digi .spots-bar select')[${sel}]; s.value = ${JSON.stringify(v)}; s.dispatchEvent(new Event('change', { bubbles: true })); })()`);
await pick(0, mode);
await sleep(300);
await pick(1, dial);
if (deep) await p.ev(`${q(".digi .spots-bar input[type=checkbox]")}.click()`);
const t0 = Date.now();
while (Date.now() - t0 < Number(secs) * 1000) {
  await sleep(5000);
  console.log(`${Math.round((Date.now() - t0) / 1000)} s:`, await p.ev(`${q(".digi-status")}?.textContent`));
}
console.log("freq:", await p.ev(`${q(".freq")}.textContent`), "mode:", await p.ev(`${q("header button.on")}?.textContent`));
console.log("count:", await p.ev(`${q(".digi .spots-bar .num")}.textContent`));
console.log("rows:\n" + (await p.ev(`[...document.querySelectorAll('.digi tbody tr')].map((r) => [...r.cells].map((c) => c.textContent).join('  ')).join('\\n')`)));
const err = await p.ev(`${q(".digi .warn")}?.textContent`);
if (err) console.log("error:", err);
await p.shot(`${out}/digi.png`);
p.close();
