// SPDX-License-Identifier: GPL-3.0-or-later
// "What is this?" in the page: open URL (tune with its hash, e.g.
// #f=10099.0&m=usb), start audio, turn ID on, then ask about each listed
// signal (its ? button) and print the answer (headline, sentences,
// figures); with DRAG=F0,F1 also a shift-drag across that fraction of the
// waterfall's width (after ZOOM wheel steps in at its middle); with WIDE=1
// an answer's Wide look (IQ) too, and the tuning after it. Screenshot
// whatis.png in OUT.
//   CHROME=... [DRAG=0.45,0.55 [ZOOM=6]] [WIDE=1] node whatis.mjs URL [PASSBAND_LABEL]
import { launch, q, sleep, decoder, lit } from "./lib.mjs";
const [url, bw = ""] = process.argv.slice(2);
const out = process.env.OUT ?? ".";
const p = await launch({ width: Number(process.env.W ?? 1400), height: Number(process.env.H ?? 900), autoplay: true });
await p.goto(url);
await sleep(3000);
await p.ev(`${q(".start")}?.click()`);
await sleep(1500);
if (bw) {
  await p.ev(`document.querySelector('.drop-bw .drop-btn')?.click()`);
  await sleep(100);
  await p.ev(`[...document.querySelectorAll('.drop-bw .drop-pop button')].find((b) => b.textContent === ${lit(bw)})?.click()`);
}
await decoder(p, "Signal ID");
await sleep(12000);

/** The card's answer once it has one (or after 15 s). */
async function answer() {
  for (let i = 0; i < 30; i++) {
    await sleep(500);
    const done = await p.ev(`!!document.querySelector('.sig-ask .sig-ask-title') || /Nothing stands out/.test(document.querySelector('.sig-ask')?.textContent ?? '')`);
    if (done) break;
  }
  await p.ev(`document.querySelector('.sig-ask details')?.setAttribute('open', '')`);
  return p.ev(`(() => {
    const c = document.querySelector('.sig-ask');
    if (!c) return '(no card)';
    const ps = [...c.querySelectorAll(':scope > p')].map((x) => x.textContent);
    const facts = [...c.querySelectorAll('.sig-facts tr')].map((r) => r.children[0].textContent + ': ' + r.children[1].textContent);
    return c.querySelector('.sig-ask-bar').textContent + '\\n  ' + ps.join('\\n  ') + '\\n    ' + facts.join('\\n    ');
  })()`);
}

const rows = Number(await p.ev(`document.querySelectorAll('.sigid tbody tr').length`));
console.log(`${rows} signals listed`);
for (let i = 0; i < rows; i++) {
  const row = await p.ev(`[...document.querySelectorAll('.sigid tbody tr')][${i}]?.textContent ?? ''`);
  await p.ev(`[...document.querySelectorAll('.sigid tbody tr')][${i}]?.querySelector('button')?.click()`);
  console.log(`--- row ${i}: ${row}`);
  console.log(await answer());
  if (process.env.WIDE) {
    const before = await p.ev(`${q(".freq")}.textContent + ' ' + [...document.querySelectorAll('.mode button.on, button.on')].map((b) => b.textContent).slice(0, 3).join(',')`);
    const has = await p.ev(`!![...document.querySelectorAll('.sig-ask button')].find((b) => b.textContent === 'Wide look')`);
    if (!has) continue;
    await p.ev(`[...document.querySelectorAll('.sig-ask button')].find((b) => b.textContent === 'Wide look').click()`);
    await sleep(1500);
    console.log(`--- wide look (was ${before}); now ${await p.ev(`${q(".freq")}.textContent`)}`);
    console.log(await answer());
    await sleep(1500);
    console.log(`--- after: ${await p.ev(`${q(".freq")}.textContent + ' ' + [...document.querySelectorAll('button.on')].map((b) => b.textContent).slice(0, 3).join(',')`)}`);
  }
}
if (process.env.DRAG) {
  const [f0, f1] = process.env.DRAG.split(",").map(Number);
  const r = JSON.parse(await p.ev("JSON.stringify(document.querySelector('canvas.waterfall').getBoundingClientRect())"));
  const y = r.y + r.height / 2;
  for (let i = 0; i < Number(process.env.ZOOM ?? 0); i++) {
    await p.mouse("mouseWheel", r.x + r.width / 2, y, { deltaX: 0, deltaY: -120 });
    await sleep(400);
  }
  await sleep(1500);
  const [x0, x1] = [r.x + r.width * f0, r.x + r.width * f1];
  // shift held (modifiers: 8)
  await p.mouse("mousePressed", x0, y, { modifiers: 8 });
  for (let i = 1; i <= 10; i++) {
    await p.mouse("mouseMoved", x0 + ((x1 - x0) * i) / 10, y, { modifiers: 8 });
    await sleep(30);
  }
  await p.mouse("mouseReleased", x1, y, { modifiers: 8 });
  console.log(`--- shift-drag ${f0}..${f1}; freq now ${await p.ev(`${q(".freq")}.textContent`)}`);
  console.log(await answer());
}
await p.shot(`${out}/whatis.png`);
p.close();
