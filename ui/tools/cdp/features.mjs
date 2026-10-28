// SPDX-License-Identifier: GPL-3.0-or-later
// The 2026-10-28 UI additions: a name (cut to 8 characters), the users
// list with a sharing and a private user (two extra WebSocket sessions),
// the hover preview, the tuning step (9 kHz), the AM button cycling
// AM/SAM/SAL/SAR/SAS (audio keeps playing through SAS's stereo frames),
// CPU and temperature in the status bar. Screenshots OUT-{hover,users,sas}.png.
//   CHROME=... node features.mjs URL OUT
import { launch, sleep, q } from "./lib.mjs";
const base = process.argv[2];
const out = process.argv[3];
// two other sessions: one sharing, one private
const other = (name, hz, share) => {
  const ws = new WebSocket(base.replace("http", "ws") + "ws");
  ws.onopen = () => {
    ws.send(JSON.stringify({ type: "hello", proto: 2, codecs: ["pcm16"] }));
    ws.send(JSON.stringify({ type: "name", name }));
    ws.send(JSON.stringify({ type: "share", on: share }));
    ws.send(JSON.stringify({ type: "tune", hz }));
  };
  ws.onmessage = () => {};
  return ws;
};
const a = other("N0CALL", 7_210_000, true);
const b = other("private1", 7_300_000, false);
const p = await launch({ width: 1400, height: 900 });
await p.goto(base + "#f=7255&m=am&z=1024&c=7255");
await sleep(3000);
await p.ev("document.querySelector('.start')?.click()");
await p.type(q("input.name"), "n0call-long");
await p.ev(`(() => { const e = ${q("input.name")}; e.dispatchEvent(new Event('change', {bubbles: true})); e.blur(); })()`);
await sleep(2500);
console.log("name input:", await p.ev(`${q("input.name")}.value`), "stored:", await p.ev("localStorage.getItem('web888.name')"));
console.log("status:", await p.ev("document.querySelector('.status').textContent"));
// hover over the waterfall
await p.mouse("mouseMoved", 900, 600, { button: "none" });
await sleep(400);
console.log("ghost:", await p.ev("(() => { const g = document.querySelector('.ghost'); return [g.style.display, g.style.left, g.style.width, g.textContent]; })()"));
await p.shot(out + "-hover.png");
// the users list
await p.ev("[...document.querySelectorAll('button')].find(b => b.textContent.startsWith('Users')).click()");
await sleep(300);
console.log("users:", await p.ev("[...document.querySelectorAll('.users-pop tr')].map(r => r.textContent).join(' | ')"));
await p.shot(out + "-users.png");
await p.ev("document.body.dispatchEvent(new PointerEvent('pointerdown', {bubbles: true}))");
// step selector: 9k, then the arrow
await p.ev(`(() => { const s = document.querySelector('header select'); s.value = '9000'; s.dispatchEvent(new Event('change', {bubbles: true})); })()`);
await sleep(200);
await p.ev("[...document.querySelectorAll('header button')].find(b => b.textContent === '▶').click()");
await sleep(600);
console.log("after ▶ with 9k:", await p.ev("document.querySelector('.freq').textContent"));
// AM cycle; the frames the player gets
for (let i = 0; i < 5; i++) {
  await p.ev("document.querySelector('button.am').click()");
  await sleep(1500);
  console.log("mode button:", await p.ev("document.querySelector('button.am').textContent"), "pb:", await p.ev("[...document.querySelectorAll('header .group')].map(g => g.textContent).join(' / ')"));
  if (i === 3) await p.shot(out + "-sas.png");
}
console.log("buffer:", await p.ev("document.querySelector('.status').textContent"));
p.close();
a.close();
b.close();
