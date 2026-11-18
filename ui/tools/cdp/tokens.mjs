// SPDX-License-Identifier: GPL-3.0-or-later
// API tokens in the admin page (ADMIN_PW): make one, the secret shown
// once, a program's request with it (fetch with Authorization: Bearer),
// the list, revoking it; phone layout. Screenshots tokens-*.png in OUT.
//   CHROME=... ADMIN_PW=... node tokens.mjs URL
import { launch, q, sleep } from "./lib.mjs";
const url = process.argv[2];
const out = process.env.OUT ?? ".";
const pw = process.env.ADMIN_PW;
if (!pw) throw new Error("set ADMIN_PW");
const p = await launch({ width: 1300, height: 900 });
await p.goto(url + "#/admin/passwords");
await sleep(1500);
await p.type(q(".admin-login input"), pw);
await p.ev("document.querySelector('.admin-login button').click()");
await sleep(1500);
await p.ev("location.hash = '#/admin/passwords'");
await sleep(1000);
console.log("card:", await p.ev("document.querySelector('.card.tokens h3')?.textContent"));
await p.type(q(".card.tokens input"), "test recorder");
await p.ev(`(() => { const s = document.querySelectorAll('.card.tokens select');
  s[0].value = 'read'; s[0].dispatchEvent(new Event('change', { bubbles: true }));
  s[1].value = '30'; s[1].dispatchEvent(new Event('change', { bubbles: true })); })()`);
await sleep(300);
await p.ev("document.querySelector('.card.tokens button[type=submit]').click()");
await sleep(1500);
const token = await p.ev("document.querySelector('.token-made input')?.value");
console.log("made:", await p.ev("document.querySelector('.token-made p')?.textContent"), "| token shape ok:", /^w888_[0-9a-f]{64}$/.test(token ?? ""));
console.log("list:", await p.ev("[...document.querySelectorAll('.card.tokens tbody tr')].map(r => r.textContent).join(' || ')"));
const status = (t) =>
  p.ev(`fetch('api/admin/sessions', { credentials: 'omit', headers: { Authorization: 'Bearer ${t}' } }).then(r => r.status)`);
const spots = (t) => p.ev(`fetch('api/spots', { credentials: 'omit', headers: { Authorization: 'Bearer ${t}' } }).then(r => r.status)`);
console.log("read token: spots", await spots(token), "| admin API", await status(token));
await p.shot(`${out}/tokens-made.png`);
await p.ev("window.confirm = () => true");
await p.ev("[...document.querySelectorAll('.card.tokens tbody button')].find(b => b.closest('tr').textContent.includes('test recorder')).click()");
await sleep(1500);
console.log("after revoke: list", await p.ev("document.querySelectorAll('.card.tokens tbody tr').length"), "| spots", await spots(token), "| made box", await p.ev("!!document.querySelector('.token-made')"));
await p.cdp("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 1, mobile: true });
await sleep(800);
console.log(
  "phone horizontal scroll:",
  await p.ev("document.documentElement.scrollWidth > window.innerWidth || document.querySelector('.page').scrollWidth > document.querySelector('.page').clientWidth"),
);
await p.shot(`${out}/tokens-phone.png`);
p.close();
