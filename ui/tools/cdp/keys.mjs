// SPDX-License-Identifier: GPL-3.0-or-later
// The key help (`?`, the status bar's "keys") at a few window sizes: does
// anything stick out of its box, and does the box fit the window?
// Screenshots in OUT.
//   CHROME=... OUT=/tmp/x node keys.mjs URL
import { resolve } from "node:path";
import { launch, q, sleep } from "./lib.mjs";
const [url] = process.argv.slice(2);
const out = resolve(process.env.OUT ?? ".");
const sizes = [
  [1400, 900],
  [1024, 640],
  [390, 844],
  [360, 640],
];
for (const [width, height] of sizes) {
  const p = await launch({ width, height });
  await p.goto(url);
  await sleep(2500);
  await p.ev(`${q(".start")}?.click()`);
  await sleep(800);
  await p.cdp("Input.dispatchKeyEvent", { type: "keyDown", key: "?", code: "Slash", text: "?" });
  await p.cdp("Input.dispatchKeyEvent", { type: "keyUp", key: "?", code: "Slash" });
  await sleep(400);
  const m = await p.ev(`(() => {
    const b = ${q(".keyhelp-box")};
    if (!b) return null;
    const r = b.getBoundingClientRect();
    // the right-most and lowest edge of anything inside the box
    let right = 0, bottom = 0;
    for (const e of b.querySelectorAll("*")) {
      const x = e.getBoundingClientRect();
      right = Math.max(right, x.right);
      bottom = Math.max(bottom, x.bottom);
    }
    return {
      box: [Math.round(r.left), Math.round(r.top), Math.round(r.right), Math.round(r.bottom)],
      sticksOutRight: Math.max(0, Math.round(right - r.right)),
      sticksOutBelow: b.scrollHeight > b.clientHeight + 1 ? 0 : Math.max(0, Math.round(bottom - r.bottom)),
      scrolls: b.scrollHeight > b.clientHeight + 1,
      fitsWindow: r.left >= 0 && r.top >= 0 && r.right <= innerWidth && r.bottom <= innerHeight,
    };
  })()`);
  console.log(`${width}x${height}:`, JSON.stringify(m));
  await p.shot(`${out}/keys-${width}x${height}.png`);
  p.close();
  await sleep(800);
}
process.exit(0);
