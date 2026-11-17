// SPDX-License-Identifier: GPL-3.0-or-later
// The admin page's operations (ADMIN_PW): Logs (each source, the filter,
// the phone layout), Configuration's Backup (the download, then a restore
// of it through the file input) and Update's Boot loader card.
// Screenshots ops-*.png in OUT (default .). Restoring writes the server's
// own settings back to its config file (unchanged).
//   CHROME=... ADMIN_PW=... OUT=... node ops.mjs URL
import { launch, q, sleep, lit } from "./lib.mjs";
import { writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
const url = process.argv[2];
const out = resolve(process.env.OUT ?? ".");
const pw = process.env.ADMIN_PW;
if (!pw) throw new Error("set ADMIN_PW");
const p = await launch({ width: 1300, height: 900 });
await p.goto(url + "#/admin");
await sleep(1500);
await p.type(q(".admin-login input"), pw);
await p.ev("document.querySelector('.admin-login button').click()");
await sleep(1500);

// Logs
await p.ev("location.hash = '#/admin/logs'");
await sleep(2000);
const sources = await p.ev("[...document.querySelectorAll('.logbar nav button')].map(b => b.textContent + (b.disabled ? ' (off)' : '')).join(' | ')");
console.log("log sources:", sources);
for (const name of ["web888d", "System (this boot)", "Kernel", "web888-setup.log"]) {
  const off = await p.ev(`[...document.querySelectorAll('.logbar nav button')].find(b => b.textContent === ${lit(name)}).disabled`);
  if (off) continue;
  await p.ev(`[...document.querySelectorAll('.logbar nav button')].find(b => b.textContent === ${lit(name)}).click()`);
  await sleep(1500);
  const n = await p.ev("document.querySelector('pre.log').textContent.split('\\n').filter(Boolean).length");
  const warn = await p.ev("document.querySelector('.logs > .warn')?.textContent ?? ''");
  console.log(`  ${name}: ${n} lines${warn ? `; ${warn}` : ""}`);
}
await p.ev("[...document.querySelectorAll('.logbar nav button')].find(b => !b.disabled).click()");
await sleep(1500);
await p.shot(join(out, "ops-logs.png"));
const before = await p.ev("document.querySelector('pre.log').textContent.split('\\n').filter(Boolean).length");
await p.type("document.querySelector('.logbar input[type=search]')", "zzz-no-such-text");
await sleep(500);
const after = await p.ev("document.querySelector('pre.log').textContent.split('\\n').filter(Boolean).length");
console.log(`filter: ${before} lines -> ${after}`);

// Backup and restore
await p.ev("location.hash = '#/admin/config'");
await sleep(2000);
console.log("cards:", await p.ev("[...document.querySelectorAll('.card h3')].map(h => h.textContent).join(' | ')"));
const backup = await p.ev(`fetch(${lit(url)} + "api/admin/backup").then(async r => ({ status: r.status, disp: r.headers.get('content-disposition'), body: await r.text() }))`);
console.log("backup:", backup.status, backup.disp, `${backup.body.length} bytes`);
const file = join(out, "ops-backup.json");
writeFileSync(file, backup.body);
await p.ev("document.querySelector('.card label.file').scrollIntoView()");
await p.ev("window.confirm = () => true");
const doc = await p.cdp("DOM.getDocument", {});
const node = await p.cdp("DOM.querySelector", { nodeId: doc.result.root.nodeId, selector: ".card label.file input[type=file]" });
await p.cdp("DOM.setFileInputFiles", { nodeId: node.result.nodeId, files: [file] });
await sleep(2000);
console.log("restore:", await p.ev("[...document.querySelectorAll('.card p.ok, .card p.warn')].map(e => e.textContent).join(' | ')"));
await p.shot(join(out, "ops-backup.png"));

// Boot loader
await p.ev("location.hash = '#/admin/update'");
await sleep(2500);
console.log("boot loader:", await p.ev("[...document.querySelectorAll('.card')].find(c => c.querySelector('h3')?.textContent === 'Boot loader')?.textContent ?? 'no card'"));
await p.shot(join(out, "ops-update.png"));

// phone
await p.cdp("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 1, mobile: true });
await p.ev("location.hash = '#/admin/logs'");
await sleep(1500);
console.log(
  "phone horizontal scroll (logs):",
  await p.ev("document.documentElement.scrollWidth > window.innerWidth || document.querySelector('.page').scrollWidth > document.querySelector('.page').clientWidth"),
);
await p.shot(join(out, "ops-logs-phone.png"));
p.close();
