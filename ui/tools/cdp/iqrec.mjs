// SPDX-License-Identifier: GPL-3.0-or-later
// IQ recording from the page: start audio, open Rec, pick IQ and the format
// and a length, wait for the file; it lands in OUT (the browser's
// download), and the page's note on it is printed. Needs a server with
// bins (the channelizer, or --bins N).
//   CHROME=... [FORMAT=cs16|wav] node iqrec.mjs URL [SECONDS: 10|30|60|120|300]
import { readdirSync, statSync } from "node:fs";
import { resolve } from "node:path";
import { launch, q, sleep } from "./lib.mjs";
const [url, secs = "10"] = process.argv.slice(2);
const out = resolve(process.env.OUT ?? ".");
const p = await launch({ autoplay: true });
await p.cdp("Page.setDownloadBehavior", { behavior: "allow", downloadPath: out });
await p.goto(url);
await sleep(3000);
await p.ev(`${q(".start")}?.click()`);
await sleep(1500);
const before = new Set(readdirSync(out));
// Rec opens a box: the IQ there, then the format and a length
const openIq = async () => {
  await p.ev(`${q(".rec-ctl .pop-btn")}.click()`);
  await sleep(300);
  await p.ev(`[...document.querySelectorAll(".rec-kind button")].find((b) => b.textContent === "IQ")?.click()`);
  await sleep(200);
};
await openIq();
const fmt = process.env.FORMAT ?? "cs16";
await p.ev(`[...document.querySelectorAll('.rec-ctl .pop-box label')].find((l) => l.textContent.trim() === '.${fmt}').querySelector('input').click()`);
await sleep(200);
await p.ev(`${q(`.rec-ctl button[data-rec="${secs}"]`)}.click()`);
const t0 = Date.now();
while (Date.now() - t0 < (Number(secs) + 20) * 1000) {
  await sleep(2000);
  const b = await p.ev(`${q("button.qrec")}?.textContent`);
  console.log(`${Math.round((Date.now() - t0) / 1000)} s: ${b ?? "stopped"} | ${await p.ev(`${q("button.qrec")}?.title`)}`);
  if (!b) break;
}
await sleep(2000);
const files = readdirSync(out).filter((f) => !before.has(f) && !f.endsWith(".crdownload"));
for (const f of files) console.log("saved:", f, statSync(`${out}/${f}`).size, "bytes");
await openIq();
console.log("note:", await p.ev(`${q(".rec-body p:last-child")}?.textContent`));
p.close();
