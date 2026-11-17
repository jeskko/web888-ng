// SPDX-License-Identifier: GPL-3.0-or-later
// The listener's Config box: the header no longer has the moved controls;
// Config opens the box; the name, sharing, data saver, waterfall colours,
// time marks and a label source each take effect (and are remembered);
// Escape closes it. Then the phone's More sheet shows the same settings.
// Screenshots in OUT.
//   CHROME=... OUT=/tmp/x node config.mjs URL
import { resolve } from "node:path";
import { launch, lit, q, sleep } from "./lib.mjs";
const [url] = process.argv.slice(2);
const out = resolve(process.env.OUT ?? ".");
const btn = (t) => `[...document.querySelectorAll("button")].find((b) => b.textContent.trim() === ${lit(t)})`;
const stored = (k) => `localStorage.getItem(${lit(`web888.${k}`)})`;

let p = await launch({ width: 1400, height: 900 });
await p.goto(url);
await sleep(3000);
await p.ev(`${q(".start")}?.click()`);
await sleep(1500);
console.log("== header");
console.log(
  "  moved out:",
  JSON.stringify(
    await p.ev(`({
      name: !!${q("header input.name")},
      shared: !!(${btn("Shared")} || ${btn("Private")}),
      labels: !!${btn("Labels")},
      saver: !!${btn("Saver")},
      palette: !!${q("footer select.palette")},
      time: !!${btn("Time")},
      config: !!${btn("Config")},
    })`),
  ),
);
console.log("== the box");
await p.ev(`${btn("Config")}.click()`);
await sleep(300);
console.log("  open:", await p.ev(`!!${q(".config-box")}`), "| sections:", await p.ev(`[...document.querySelectorAll(".config-box h4")].map((h) => h.textContent).join(", ")`));
// the name: typed, then committed on change
await p.type(q(".config-box input.name"), "N0CALL");
await p.ev(`(() => { const e = ${q(".config-box input.name")}; e.dispatchEvent(new Event("change", { bubbles: true })); e.blur(); })()`);
// sharing off, the saver on, the heat palette, time marks on, the utility labels toggled
const share0 = await p.ev(`${q(".config-box input.share")}?.checked`);
await p.ev(`${q(".config-box input.share")}.click()`);
await p.ev(`${q(".config-box input.saver")}.click()`);
await p.ev(`(() => { const s = ${q(".config-box select.palette")}; s.value = "heat"; s.dispatchEvent(new Event("change", { bubbles: true })); })()`);
const times0 = await p.ev(`${q(".config-box input.times")}.checked`);
await p.ev(`${q(".config-box input.times")}.click()`);
const labels0 = await p.ev(stored("labels"));
await p.ev(`${q(".config-box .labels-opts input[type=checkbox]")}.click()`);
await sleep(1500);
console.log("  name:", await p.ev(stored("name")));
console.log("  share:", share0, "->", await p.ev(`${q(".config-box input.share")}.checked`), "| users list says private:", await p.ev(`(() => { ${btn("Users 1")}?.click(); return document.querySelector(".users-pop .me")?.textContent ?? null; })()`));
console.log("  saver:", await p.ev(stored("dataSaver")), "| palette:", await p.ev(stored("palette")), "| time marks:", times0, "->", await p.ev(stored("wfTimes")));
console.log("  labels:", labels0, "->", await p.ev(stored("labels")));
await p.shot(`${out}/config-desktop.png`);
await p.cdp("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape" });
await p.cdp("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape" });
await sleep(300);
console.log("  closed by Escape:", !(await p.ev(`!!${q(".config-box")}`)));
// put them back for the next run
await p.ev(`(() => { ["name", "dataSaver", "palette", "wfTimes", "labels"].forEach((k) => localStorage.removeItem("web888." + k)); })()`);
p.close();
await sleep(800);

console.log("== phone");
p = await launch({ width: 390, height: 844 });
await p.cdp("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 1, mobile: true });
await p.cdp("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 });
await p.goto(url + "?ui=mobile");
await sleep(4000);
await p.ev(`${btn("⋯ More")}?.click()`);
await sleep(800);
console.log("  in More:", await p.ev(`[...document.querySelectorAll(".m-sheet .user-config h4")].map((h) => h.textContent).join(", ")`));
const ow = await p.ev("(() => { const b = document.querySelector('.m-sheet-body'); return b ? [b.scrollWidth, b.clientWidth] : null; })()");
console.log("  sheet width (scroll, client):", JSON.stringify(ow));
await p.ev(`document.querySelector(".m-sheet .user-config")?.scrollIntoView()`);
await sleep(300);
await p.shot(`${out}/config-phone.png`);
p.close();
process.exit(0);
