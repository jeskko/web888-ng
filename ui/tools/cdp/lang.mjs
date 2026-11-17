// SPDX-License-Identifier: GPL-3.0-or-later
// The page in each language (dist/i18n/index.json): the header and footer
// don't overflow, how much of the visible text changed from English, and
// screenshots of the page and the Config box. LANGS="fi de" picks some.
//   CHROME=... OUT=/tmp/x node lang.mjs URL
import { resolve } from "node:path";
import { launch, q, sleep } from "./lib.mjs";
const [url] = process.argv.slice(2);
const out = resolve(process.env.OUT ?? ".");
const index = await (await fetch(new URL("i18n/index.json", url))).json();
const langs = process.env.LANGS ? process.env.LANGS.split(" ") : ["en", ...index.map((l) => l.code)];
let english = null;
for (const code of langs) {
  const p = await launch({ width: 1400, height: 900 });
  await p.goto(url);
  await p.ev(`localStorage.setItem("web888.lang", ${JSON.stringify(code)})`);
  await p.goto(url);
  await sleep(3000);
  const m = await p.ev(`(() => {
    const over = (sel) => { const e = document.querySelector(sel); return e ? Math.max(0, e.scrollWidth - e.clientWidth) : null; };
    const labels = [...document.querySelectorAll("header button, footer button, header .drop-btn")].map((b) => b.textContent.trim()).filter(Boolean);
    return { lang: document.documentElement.lang, header: over("header"), footer: over("footer .controls"), status: over(".status"), labels };
  })()`);
  if (code === "en") english = m.labels;
  const same = english ? m.labels.filter((l, i) => l === english[i]).length : 0;
  console.log(`${code}: html lang=${m.lang}, overflow header ${m.header} footer ${m.footer} status ${m.status}; ${m.labels.length} buttons, ${same} as in English`);
  console.log(`   ${m.labels.join(" | ")}`);
  await p.shot(`${out}/lang-${code}.png`);
  await p.ev(`${q("header .tools .group:last-child button")}?.click()`);
  await sleep(300);
  await p.shot(`${out}/lang-${code}-config.png`);
  p.close();
  await sleep(800);
}
process.exit(0);
