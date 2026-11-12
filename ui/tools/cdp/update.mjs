// SPDX-License-Identifier: GPL-3.0-or-later
// The admin page's Update tab (OS updates, on the web888-ng-os image: in
// the emulator, hw/emu): log in, look for a release, optionally start an
// install (ACT=install: it really installs into the other root slot) or
// reboot (ACT=reboot); prints the cards and saves update.png in OUT.
//   CHROME=... ADMIN_PW=... [ACT=check|install|reboot] node update.mjs URL
import { launch, q, sleep } from "./lib.mjs";
const url = process.argv[2];
const out = process.env.OUT ?? ".";
const pw = process.env.ADMIN_PW;
if (!pw) throw new Error("set ADMIN_PW");
const act = process.env.ACT ?? "check";
const p = await launch({ width: 1300, height: 900 });
// a page from before a reboot may be cached
await p.cdp("Network.enable");
await p.cdp("Network.setCacheDisabled", { cacheDisabled: true });
await p.goto(url + "#/admin/update");
await sleep(2000);
if (await p.ev("!!document.querySelector('.admin-login input')")) {
  await p.type(q(".admin-login input"), pw);
  await p.ev("document.querySelector('.admin-login button').click()");
  await sleep(2500);
}
await p.ev("location.hash = '#/admin/update'");
await sleep(2500);
await p.ev("window.confirm = () => true");
const click = async (text) =>
  console.log(
    `${text}:`,
    await p.ev(
      `(() => { const b = [...document.querySelectorAll('.card button, .card a')].find(b => b.textContent.startsWith(${JSON.stringify(text)})); if (!b) return false; b.click(); return true; })()`,
    ),
  );
await click("Look for");
await sleep(8000);
if (act === "install") {
  await click("Install");
  await sleep(30_000);
}
if (act === "reboot") {
  await click("Reboot now");
  await sleep(5000);
}
console.log("cards:", await p.ev("[...document.querySelectorAll('.card')].map(c => c.innerText.replace(/\\s+/g, ' ')).join(' || ')"));
await p.shot(`${out}/update.png`);
p.close();
