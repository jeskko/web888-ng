// SPDX-License-Identifier: GPL-3.0-or-later
// The idle limit (a server with [limits] idle_minutes = 1): the page sits
// idle; after a minute it must show "Session ended" and stay ended, and
// Reconnect must bring it back. Takes about 90 s.
//   CHROME=... node idle.mjs URL
import { launch, sleep } from "./lib.mjs";
const url = process.argv[2];
const conn = "document.querySelector('.conn')?.textContent";
const p = await launch({ width: 1000, height: 700 });
await p.goto(url);
await sleep(3000);
console.log("t=3s conn:", await p.ev(conn));
await sleep(72000);
console.log("t=75s conn:", await p.ev(conn), "| overlay:", await p.ev("document.querySelector('.overlay')?.textContent"));
await sleep(8000);
console.log("t=83s still:", await p.ev(conn));
await p.ev("[...document.querySelectorAll('.overlay button')].find(b => b.textContent === 'Reconnect').click()");
await sleep(3000);
console.log("after Reconnect:", await p.ev(conn));
p.close();
