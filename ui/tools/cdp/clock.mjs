// SPDX-License-Identifier: GPL-3.0-or-later
// The admin page's Clock tab (ADMIN_PW): its cards and their text, a
// screenshot admin-clock.png (and admin-clock-phone.png) in OUT.
//   CHROME=... ADMIN_PW=... [FAKE=clock.json] node clock.mjs URL
import { launch, q, sleep } from "./lib.mjs";
const url = process.argv[2];
const out = process.env.OUT ?? ".";
const pw = process.env.ADMIN_PW;
if (!pw) throw new Error("set ADMIN_PW");
const p = await launch({ width: 1300, height: 1000 });
// FAKE=file.json: the page gets that as api/admin/clock (to see a full
// view from a server without chrony or bins)
if (process.env.FAKE) {
  const body = (await import("node:fs")).readFileSync(process.env.FAKE, "utf8");
  await p.cdp("Page.addScriptToEvaluateOnNewDocument", {
    source: `{ const f = window.fetch; window.fetch = (u, o) => String(u).includes("api/admin/clock")
      ? Promise.resolve(new Response(${JSON.stringify(body)}, { headers: { "content-type": "application/json" } }))
      : f(u, o); }`,
  });
}
await p.goto(url + "#/admin/clock");
await sleep(1500);
await p.type(q(".admin-login input"), pw);
await p.ev("document.querySelector('.admin-login button').click()");
await sleep(4000);
console.log("tab:", await p.ev("document.querySelector('nav button.on, .tabs button.on')?.textContent"));
console.log(await p.ev("[...document.querySelectorAll('.card')].map(c => c.innerText.replace(/\\n+/g, ' | ')).join('\\n\\n')"));
await p.shot(`${out}/admin-clock.png`);
await p.cdp("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 1, mobile: true });
await sleep(1000);
console.log("phone: page wider than the screen:", await p.ev("document.documentElement.scrollWidth > innerWidth"));
await p.shot(`${out}/admin-clock-phone.png`);
p.close();
