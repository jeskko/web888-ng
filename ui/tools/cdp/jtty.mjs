// SPDX-License-Identifier: GPL-3.0-or-later
// JTTY decoding in the page: start audio, turn JTTY on, pick a dial from
// its list, wait, print the decoded rows; saves jtty.png in OUT.
//   CHROME=... node jtty.mjs URL [DIAL_HZ] [SECONDS]
import { launch, q, sleep } from "./lib.mjs";
const [url, dial = "14090000", secs = "70"] = process.argv.slice(2);
const out = process.env.OUT ?? ".";
const p = await launch({ width: Number(process.env.W ?? 1400), height: Number(process.env.H ?? 900), autoplay: true });
await p.goto(url);
await sleep(3000);
console.log("secure:", await p.ev("window.isSecureContext"));
await p.ev(`${q(".start")}?.click()`);
await sleep(2000);
await p.ev(`[...document.querySelectorAll('.controls button')].find((b) => b.textContent === 'JTTY').click()`);
await sleep(500);
await p.ev(`(() => { const s = ${q(".jtty select")}; s.value = ${JSON.stringify(dial)}; s.dispatchEvent(new Event('change', { bubbles: true })); })()`);
await sleep(Number(secs) * 1000);
console.log("freq:", await p.ev(`${q(".freq")}.textContent`), "mode:", await p.ev(`${q("header button.on")}?.textContent`));
console.log("status:", await p.ev(`${q(".status")}.textContent`));
console.log("rows:", await p.ev(`[...document.querySelectorAll('.jtty tbody tr')].map((r) => r.textContent).join(' | ')`));
console.log("panel:", await p.ev(`${q(".jtty .spots-list")}.textContent.slice(0, 300)`));
await p.shot(`${out}/jtty.png`);
p.close();
