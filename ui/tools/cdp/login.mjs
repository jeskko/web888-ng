// SPDX-License-Identifier: GPL-3.0-or-later
// The listener-password form (a server with a listener password): a wrong
// password, then the right one (LISTENER_PW); the page must connect.
//   CHROME=... LISTENER_PW=... node login.mjs URL
import { launch, sleep } from "./lib.mjs";
const url = process.argv[2];
const pw = process.env.LISTENER_PW;
if (!pw) throw new Error("set LISTENER_PW");
const form = "document.querySelector('.login input')";
const p = await launch({ width: 1000, height: 700 });
await p.goto(url);
await sleep(2000);
console.log("form shown:", await p.ev("!!document.querySelector('.login')"), "| conn:", await p.ev("document.querySelector('.conn')?.textContent"));
await p.type(form, "wrong-password");
await p.ev("document.querySelector('.login button').click()");
await sleep(1500);
console.log("after wrong:", await p.ev("document.querySelector('.login .warn')?.textContent"));
await p.type(form, pw);
await p.ev("document.querySelector('.login button').click()");
await sleep(3000);
console.log(
  "after right: form",
  await p.ev("!!document.querySelector('.login')"),
  "| conn:",
  await p.ev("document.querySelector('.conn')?.textContent"),
  "| freq:",
  await p.ev("document.querySelector('.freq')?.textContent"),
);
p.close();
