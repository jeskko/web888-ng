// SPDX-License-Identifier: GPL-3.0-or-later
// IQ mode, the audio spectrum, the info page, and the admin's ADC
// switches, info photo and Ban; with GUEST=1 the visitor flow instead (a
// receiver with a listener password and `[limits] open_channels = 1`:
// the first visitor listens, the second queues, then logs in).
//   CHROME=... ADMIN_PW=... OUT=/tmp/x node access.mjs URL
//   CHROME=... LISTENER_PW=... GUEST=1 OUT=/tmp/x node access.mjs URL
import { readdirSync, statSync } from "node:fs";
import { resolve } from "node:path";
import { launch, q, sleep } from "./lib.mjs";
const [url] = process.argv.slice(2);
const out = resolve(process.env.OUT ?? ".");
const btn = (text, root = "document") =>
  `[...${root}.querySelectorAll("button")].find((b) => b.textContent.trim() === ${JSON.stringify(text)})`;

if (process.env.GUEST) {
  const p = await launch({ autoplay: true });
  await p.goto(url);
  await sleep(3000);
  await p.ev(`${q(".start")}?.click()`);
  await sleep(1500);
  console.log("visitor 1:", await p.ev(`${q(".status .right")}?.textContent`), "| login form:", await p.ev(`!!${q(".login")}`), "| banner:", await p.ev(`${q(".banner")}?.textContent ?? null`));
  // a second visitor (another browser): waits for the one open receiver
  process.env.CDP_PORT = String(Number(process.env.CDP_PORT ?? 9333) + 1);
  const p2 = await launch({ autoplay: true });
  await p2.goto(url);
  await sleep(3000);
  console.log("visitor 2 banner:", await p2.ev(`${q(".banner")}?.textContent`));
  await p2.shot(`${out}/access-guest-queue.png`);
  await p2.ev(`[...document.querySelectorAll(".banner button")].find((b) => b.textContent === "Log in")?.click()`);
  await sleep(500);
  console.log("visitor 2 login form:", await p2.ev(`${q(".login p")}?.textContent`));
  await p2.type(q(".login input"), process.env.LISTENER_PW ?? "");
  await p2.ev(`${q(".login button[type=submit]")}.click()`);
  await sleep(4000);
  console.log("visitor 2 after login: banner", JSON.stringify(await p2.ev(`${q(".banner")}?.textContent ?? null`)), "| status", await p2.ev(`${q(".status .right")}?.textContent`));
  p2.close();
  p.close();
  process.exit(0);
}

const p = await launch({ autoplay: true });
await p.cdp("Page.setDownloadBehavior", { behavior: "allow", downloadPath: out });
await p.goto(url);
await sleep(3000);
await p.ev(`${q(".start")}?.click()`);
await sleep(1500);

console.log("== IQ and the audio spectrum");
await p.ev(`${btn("IQ")}.click()`);
await p.ev(`${btn("AF")}.click()`);
await sleep(4000);
console.log("  status:", await p.ev(`[...document.querySelectorAll(".status span")].map((s) => s.textContent).find((t) => /kbit/.test(t) && !/^wf/.test(t))`));
let seen = new Set(readdirSync(out));
await p.ev(`${btn("Rec ▾")}.click()`);
await sleep(200);
await p.ev(`${q('.iqrec-pop button[data-arec="60"]')}.click()`);
await sleep(3000);
await p.ev(`${q("button.arec")}.click()`);
await sleep(2500);
for (const f of readdirSync(out).filter((f) => !seen.has(f) && f.endsWith(".wav"))) {
  const b = (await import("node:fs")).readFileSync(`${out}/${f}`);
  console.log("  saved:", f, statSync(`${out}/${f}`).size, "bytes, channels", b.readUInt16LE(22), "rate", b.readUInt32LE(24));
}
await p.shot(`${out}/access-iq-af.png`);

console.log("== info page");
await p.ev(`${btn("Info")}.click()`);
await sleep(3000);
console.log("  ", (await p.ev(`${q(".info-box")}?.textContent`))?.slice(0, 300));

console.log("== admin");
// a second browser: its own DevTools port
process.env.CDP_PORT = String(Number(process.env.CDP_PORT ?? 9333) + 1);
const a = await launch({ autoplay: false });
await a.goto(new URL("#/admin", url).href);
await sleep(2000);
await a.type(q("input[type=password]"), process.env.ADMIN_PW ?? "");
await a.ev(`${q("form button[type=submit]")}?.click()`);
await sleep(2000);
await a.ev(`[...document.querySelectorAll("label")].find((l) => l.textContent.includes("dither"))?.querySelector("input").click()`);
await sleep(2500);
console.log("  ADC:", await a.ev(`[...document.querySelectorAll("dt")].find((d) => d.textContent === "ADC")?.nextElementSibling.textContent`), "| dither checked:", await a.ev(`[...document.querySelectorAll("label")].find((l) => l.textContent.includes("dither"))?.querySelector("input").checked`));
await a.goto(new URL("#/admin/info", url).href);
await sleep(1500);
const doc = await a.cdp("DOM.getDocument");
const node = await a.cdp("DOM.querySelector", { nodeId: doc.result.root.nodeId, selector: "input[type=file]" });
await a.cdp("DOM.setFileInputFiles", { nodeId: node.result.nodeId, files: [`${out}/access-iq-af.png`] });
await sleep(2000);
console.log("  photo:", await a.ev(`${q(".card .ok, .card .warn")}?.textContent`), "| img", await a.ev(`!!${q(".info-photo")}`));
await a.shot(`${out}/access-admin-info.png`);
// the listener's info page with the photo
await p.ev(`${btn("✕", "document.querySelector('.info-box')")}.click()`);
await p.ev(`${btn("Info")}.click()`);
await sleep(1500);
await p.shot(`${out}/access-info.png`);
console.log("  listener sees the photo:", await p.ev(`${q(".info-box img")}?.complete`));
await p.ev(`${btn("✕", "document.querySelector('.info-box')")}.click()`);

console.log("== ban");
await a.goto(new URL("#/admin/sessions", url).href);
await sleep(2500);
await a.cdp("Page.setInterceptFileChooserDialog", { enabled: false });
// confirm() answers yes
await a.ev(`window.confirm = () => true`);
await a.ev(`[...document.querySelectorAll("tr")].find((r) => r.textContent.includes("web"))?.querySelector("button[title^='End its']").click()`);
await sleep(3000);
console.log("  listener page:", await p.ev(`${q(".overlay p")}?.textContent`), "|", await p.ev(`${q(".status .conn")}?.textContent`));
await p.ev(`location.reload()`);
await sleep(3000);
console.log("  after reload:", await p.ev(`${q(".overlay p")}?.textContent`));
a.close();
p.close();
