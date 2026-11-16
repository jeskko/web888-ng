// SPDX-License-Identifier: GPL-3.0-or-later
// Time-code stations in the page: start audio, turn Time on, tune a
// station from its list ("DCF77|77500"; "-" keeps the URL's tuning), print
// the panel (status, this minute's seconds, the minutes read) every 30 s;
// saves timecode.png in OUT.
//   CHROME=... node timecode.mjs URL [STATION|CARRIER] [SECONDS]
import { decoder, launch, q, sleep } from "./lib.mjs";
const [url, station = "DCF77|77500", secs = "300"] = process.argv.slice(2);
const out = process.env.OUT ?? ".";
const p = await launch({ autoplay: true });
await p.goto(url);
await sleep(3000);
await p.ev(`${q(".start")}?.click()`);
await sleep(1500);
await decoder(p, "Time");
await sleep(500);
if (station !== "-")
  await p.ev(`(() => { const s = [...document.querySelectorAll('.tc .spots-bar select')][1]; s.value = ${JSON.stringify(station)}; s.dispatchEvent(new Event('change', { bubbles: true })); })()`);
const t0 = Date.now();
while (Date.now() - t0 < Number(secs) * 1000) {
  await sleep(30000);
  console.log(`--- ${Math.round((Date.now() - t0) / 1000)} s`);
  console.log(await p.ev(`[...document.querySelectorAll('.tc .spots-list > p, .tc .tc-secs, .tc .tc-min tr')].map((e) => e.textContent).join('\\n')`));
}
console.log("freq:", await p.ev(`${q(".freq")}.textContent`), "mode:", await p.ev(`${q("header button.on")}?.textContent`));
await p.shot(`${out}/timecode.png`);
p.close();
