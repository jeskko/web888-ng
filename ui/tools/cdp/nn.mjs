// SPDX-License-Identifier: GPL-3.0-or-later
// The neural noise reduction (NN): off in AM (disabled), on in USB and
// LSB, the strength slider, nr.wasm fetched once; with nr-models.json in
// the UI directory, each model of the selector in turn; prints the button's
// state (strength 50 % and 250 %), the player's underruns and page errors
// at each step, and saves nn.png in OUT (default .).
//   CHROME=... node nn.mjs URL
import { launch, sleep, lit } from "./lib.mjs";
const url = process.argv[2];
const out = process.env.OUT ?? ".";
const p = await launch({ autoplay: true });
await p.goto(url);
await sleep(3000);
await p.ev("document.querySelector('.start')?.click()");
await sleep(2000);
const btn = (label) => `[...document.querySelectorAll("button")].find((b) => b.textContent.trim() === ${lit(label)})`;
const mode = async (m) => {
  await p.ev(`${btn(m)}?.click()`);
  await sleep(1500);
};
const report = async (what) => {
  await sleep(4000);
  const s = await p.ev("document.querySelector('.status').textContent");
  const under = s.match(/(\d+)\s*underruns/);
  const b = await p.ev(`(() => { const b = ${btn("NN")}; return b ? [b.disabled, b.classList.contains("on"), b.title] : null })()`);
  const slider = await p.ev(`!!document.querySelector('input[aria-label="Neural noise reduction strength"]')`);
  const wasm = await p.ev(`performance.getEntriesByType("resource").filter((e) => e.name.endsWith("/nr.wasm")).length`);
  console.log(`${what}: NN ${b ? `${b[0] ? "disabled" : "enabled"}, ${b[1] ? "on" : "off"}` : "missing"}, slider ${slider}, nr.wasm fetched ${wasm}x, underruns ${under?.[1] ?? "?"}`);
};
await mode("AM");
await report("AM");
await mode("USB");
await report("USB");
await p.ev(`${btn("NN")}.click()`);
await report("USB, NN clicked");
await p.ev(`(() => { const r = document.querySelector('input[aria-label="Neural noise reduction strength"]'); r.value = "0.5"; r.dispatchEvent(new Event("input", { bubbles: true })); })()`);
await report("strength 50 %");
await p.ev(`(() => { const r = document.querySelector('input[aria-label="Neural noise reduction strength"]'); r.value = "2.5"; r.dispatchEvent(new Event("input", { bubbles: true })); })()`);
await report("strength 250 %");
const models = await p.ev(`[...(document.querySelector('select[aria-label="Neural noise reduction model"]')?.options ?? [])].map((o) => o.value)`);
for (const m of models) {
  await p.ev(`(() => { const s = document.querySelector('select[aria-label="Neural noise reduction model"]'); s.value = ${lit(m)}; s.dispatchEvent(new Event("change", { bubbles: true })); })()`);
  await report(`model ${m || "built-in"}`);
  console.log("  weights fetched:", await p.ev(`performance.getEntriesByType("resource").filter((e) => e.name.includes("nrnet-")).map((e) => e.name.split("/").pop()).join(" ") || "none"`));
}
await mode("LSB");
await report("LSB");
await p.shot(`${out}/nn.png`);
await mode("AM");
await report("AM again");
await mode("USB");
await p.ev(`${btn("NN")}.click()`);
await report("USB, NN off");
console.log("errors:", await p.ev("window.__errors?.length ?? 'n/a'"));
p.close();
