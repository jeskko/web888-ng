// SPDX-License-Identifier: GPL-3.0-or-later
// The header on one row: a fullscreen browser on a 14" MacBook Pro at its
// default scaling (1512 x 982, 2x) in every UI language; prints the
// header's rows and the width it would need unwrapped, exits 1 if any
// language wraps.
//   CHROME=... node fit.mjs URL [WIDTH HEIGHT DPR]
import { launch, sleep } from "./lib.mjs";
const [url, w = "1512", h = "982", dpr = "2"] = process.argv.slice(2);
let bad = 0;
for (const lang of ["en", "fi", "sv", "de"]) {
  const p = await launch({ width: +w, height: +h, dpr: +dpr });
  await p.goto(url);
  await sleep(1500);
  await p.ev(`localStorage.setItem("web888.lang", "${lang}")`);
  await p.goto(url);
  await sleep(5000);
  const r = JSON.parse(
    await p.ev(`(() => {
      const hd = document.querySelector("header");
      const rows = new Set([...hd.children].map((e) => Math.round(e.getBoundingClientRect().top / 10))).size;
      // unwrapped: no wrapping, nothing shrinks (widths as drawn, zoom included)
      const s = document.createElement("style");
      s.textContent = "header{flex-wrap:nowrap!important} header>*{flex-shrink:0!important} header .group{flex-wrap:nowrap!important}";
      document.head.append(s);
      const kids = [...hd.children];
      const z = parseFloat(getComputedStyle(hd).zoom) || 1;
      const cs = getComputedStyle(hd);
      const need = kids.reduce((a, e) => a + e.getBoundingClientRect().width, 0) + (kids.length - 1) * parseFloat(cs.columnGap) * z + (parseFloat(cs.paddingLeft) + parseFloat(cs.paddingRight)) * z;
      s.remove();
      return JSON.stringify({ rows, need: Math.round(need), lang: document.documentElement.lang });
    })()`),
  );
  const ok = r.rows === 1;
  if (!ok) bad++;
  console.log(`${lang}: ${ok ? "one row" : `${r.rows} rows`}, needs ${r.need} of ${w} px${r.lang !== lang ? ` (page language ${r.lang}!)` : ""}`);
  p.close();
}
process.exit(bad ? 1 : 0);
