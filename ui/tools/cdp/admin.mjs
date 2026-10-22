// SPDX-License-Identifier: GPL-3.0-or-later
// The admin pages (ADMIN_PW): log in, a receiver in an iframe (same
// cookie) so a session exists, status, sessions, config edits (checked
// as you type, an invalid value, a saved change), passwords, and the
// phone layout. Screenshots admin-*.png in OUT (default .).
// It saves limits.per_ip = PER_IP (default 6) to the server's config file.
//   CHROME=... ADMIN_PW=... node admin.mjs URL
import { launch, q, sleep } from "./lib.mjs";
const url = process.argv[2];
const out = process.env.OUT ?? ".";
const pw = process.env.ADMIN_PW;
if (!pw) throw new Error("set ADMIN_PW");
const perIp = process.env.PER_IP ?? "6";
const field = (name) =>
  `[...document.querySelectorAll('.field')].find(f => f.querySelector('.name').title === ${JSON.stringify(name)}).querySelector('input')`;
const savebar = () => p.ev("document.querySelector('.savebar').textContent");
const p = await launch({ width: 1300, height: 900 });
await p.goto(url + "#/admin");
await sleep(1500);
await p.type(q(".admin-login input"), pw);
await p.ev("document.querySelector('.admin-login button').click()");
await sleep(1500);
await p.ev(`(() => { const f = document.createElement("iframe"); f.src = ${JSON.stringify(url)};
  f.style.cssText = "position:fixed;width:400px;height:300px;right:0;bottom:0;opacity:0;pointer-events:none";
  document.body.appendChild(f); })()`);
await sleep(5000);
console.log("receiver frame:", await p.ev("document.querySelector('iframe').contentDocument.querySelector('.status')?.textContent"));
console.log("status cards:", await p.ev("[...document.querySelectorAll('.card h3')].map(h => h.textContent).join(' | ')"));
await p.shot(`${out}/admin-status.png`);
await p.ev("location.hash = '#/admin/sessions'");
await sleep(2500);
console.log("sessions:", await p.ev("[...document.querySelectorAll('table.wide tbody tr')].map(r => r.textContent).join(' || ')"));
await p.shot(`${out}/admin-sessions.png`);
await p.ev("location.hash = '#/admin/config'");
await sleep(1500);
console.log("sections:", await p.ev("[...document.querySelectorAll('.config > div > fieldset > legend')].map(l => l.textContent).join(', ')"));
console.log("'running' tags:", await p.ev("document.querySelectorAll('.tag:not(.edit)').length"));
await p.type(field("limits.per_ip"), "x");
await sleep(600);
console.log("per_ip = x:", await savebar(), "| save disabled:", await p.ev("document.querySelector('.savebar button').disabled"));
await p.type(field("limits.per_ip"), perIp);
await sleep(1200);
console.log(`per_ip = ${perIp}:`, await savebar());
await p.type(field("web.listen"), "0.0.0.0:8080");
await sleep(1200);
console.log("listen changed:", await savebar());
await p.shot(`${out}/admin-config.png`);
await p.ev("[...document.querySelectorAll('.savebar button')].find(b => b.textContent === 'Revert').click()");
await sleep(1000);
await p.type(field("limits.per_ip"), perIp);
await sleep(1000);
await p.ev("document.querySelector('.savebar button').click()");
await sleep(1500);
console.log("after save:", await savebar());
await p.ev("location.hash = '#/admin/passwords'");
await sleep(1000);
await p.shot(`${out}/admin-passwords.png`);
await p.cdp("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 1, mobile: true });
await p.ev("location.hash = '#/admin/config'");
await sleep(1500);
console.log(
  "phone horizontal scroll:",
  await p.ev("document.documentElement.scrollWidth > window.innerWidth || document.querySelector('.page').scrollWidth > document.querySelector('.page').clientWidth"),
);
await p.shot(`${out}/admin-config-phone.png`);
p.close();
