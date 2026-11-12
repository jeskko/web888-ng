// SPDX-License-Identifier: GPL-3.0-or-later
// The phone layout's panels at 390 x 844 with touch events: More, then
// Spots, Map (a tap shows a station, a second tap tunes to it) and the
// FT8/FT4/WSPR panel, Labels and the status bar; prints what it finds and
// saves 0-main.png ... 6-status.png in OUT.
//   CHROME=... node phone.mjs URL OUT
import { launch, sleep } from "./lib.mjs";
const [url, out] = process.argv.slice(2);
const p = await launch({ width: 390, height: 844 });
await p.cdp("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 1, mobile: true });
await p.cdp("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 });
await p.cdp("Network.enable");
await p.cdp("Network.setCacheDisabled", { cacheDisabled: true });
await p.goto(url + "?ui=mobile");
await sleep(5000);
const btn = (t) => `[...document.querySelectorAll('button')].find(b => b.textContent.trim() === ${JSON.stringify(t)})`;
const click = async (t) => console.log(`click ${t}:`, await p.ev(`(() => { const b = ${btn(t)}; if (!b) return false; b.click(); return true; })()`));
const off = (t) => p.ev(`(() => { const b = ${btn(t)}; if (b && b.classList.contains('on')) b.click(); })()`);
const overflow = () => p.ev("(() => { const b = document.querySelector('.m-sheet-body'); return b ? [b.scrollWidth, b.clientWidth] : null; })()");
await p.shot(`${out}/0-main.png`);
await click("⋯ More");
await sleep(800);
for (const t of ["Spots", "Map", "FT8/FT4/WSPR", "JTTY"]) await off(t);
await sleep(500);
console.log("More buttons:", await p.ev("[...document.querySelectorAll('.m-sheet .m-panels button')].map(b => b.textContent).join('|')"));
await p.shot(`${out}/1-more.png`);
const scrollTo = (sel) => p.ev(`document.querySelector('${sel}')?.scrollIntoView({block: 'start'})`);
// spots
await click("Spots");
await sleep(2500);
await scrollTo(".m-sheet .spots");
await sleep(300);
console.log("spots bar:", await p.ev("[...document.querySelectorAll('.m-sheet .spots-bar select')].map(s => s.value + '/' + s.options.length).join(' ')"), "overflow:", await overflow());
await p.shot(`${out}/2-spots.png`);
await off("Spots");
// map
await click("Map");
await sleep(4000);
await scrollTo(".m-sheet .map");
await sleep(300);
console.log("map overflow:", await overflow(), "bar h:", await p.ev("document.querySelector('.m-sheet .map .spots-bar')?.offsetHeight"), "canvas:", await p.ev("(() => { const c = document.querySelector('.m-sheet .map canvas'); return c ? [c.clientWidth, c.clientHeight] : null; })()"));
await p.shot(`${out}/3-map.png`);
// a tap on the map's busiest point (touch)
const rc = await p.ev("(() => { const c = document.querySelector('.m-sheet .map canvas'); const r = c.getBoundingClientRect(); return [r.left, r.top, r.width, r.height]; })()");
const tap = async (x, y) => {
  await p.cdp("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [{ x, y }] });
  await p.cdp("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
  await sleep(250);
};
let hit = null;
for (let j = 1; j < 12 && !hit; j++)
  for (let i = 1; i < 10 && !hit; i++) {
    const x = rc[0] + (rc[2] * i) / 10, y = rc[1] + (rc[3] * j) / 12;
    await tap(x, y);
    if (await p.ev("!!document.querySelector('.map-tip')")) hit = [x, y];
  }
console.log("tip after tap:", hit, await p.ev("document.querySelector('.map-tip')?.innerText?.replace(/\\s+/g, ' ') ?? null"));
const tipBox = await p.ev("(() => { const t = document.querySelector('.map-tip'); const c = document.querySelector('.m-sheet .map-box'); if (!t) return null; const a = t.getBoundingClientRect(), b = c.getBoundingClientRect(); return [Math.round(a.left - b.left), Math.round(b.right - a.right)]; })()");
console.log("tip inside (left, right margins):", tipBox);
await p.shot(`${out}/4-map-tap.png`);
const f0 = await p.ev("document.querySelector('.m-freq').textContent");
if (hit) { await tap(hit[0], hit[1]); await sleep(1000); }
console.log("freq before/after 2nd tap:", f0, "->", await p.ev("document.querySelector('.m-freq').textContent"));
await off("Map");
// digi
await click("FT8/FT4/WSPR");
await sleep(2000);
await scrollTo(".m-sheet .spots");
console.log("digi overflow:", await overflow());
await p.shot(`${out}/5-digi.png`);
await off("FT8/FT4/WSPR");
// labels and status
await p.ev("document.querySelector('.m-sheet-body').scrollTo(0, 0)");
console.log("labels:", await p.ev("document.querySelector('.m-sheet .labels-pop, .m-sheet .labels')?.innerText?.replace(/\\s+/g, ' ') ?? null"));
await p.ev("document.querySelector('.m-sheet .status')?.scrollIntoView({block: 'end'})");
console.log("status:", await p.ev("document.querySelector('.m-sheet .status')?.innerText?.replace(/\\s+/g, ' ')"));
await p.shot(`${out}/6-status.png`);
p.close();
