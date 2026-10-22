// SPDX-License-Identifier: GPL-3.0-or-later
// Audio soak: start audio and print the status bar (buffer, underruns,
// drops) every minute.
//   CHROME=... node soak.mjs URL [minutes, default 10]
import { launch, sleep } from "./lib.mjs";
const [url, mins = "10"] = process.argv.slice(2);
const p = await launch({ autoplay: true });
await p.goto(url);
await sleep(3000);
await p.ev("document.querySelector('.start')?.click()");
for (let m = 1; m <= Number(mins); m++) {
  await sleep(60000);
  console.log(`${m} min:`, await p.ev("document.querySelector('.status').textContent"));
}
p.close();
