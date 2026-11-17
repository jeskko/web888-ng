// SPDX-License-Identifier: GPL-3.0-or-later
// The listener tools: palettes, peak hold, waterfall time marks, a PNG
// snapshot, the key help, memories (store, recall, export, import), VFO
// A/B, audio recording to WAV, the custom AGC speed. Prints what it sees,
// saves the downloads and screenshots in OUT.
//   CHROME=... OUT=/tmp/x node listener.mjs URL
import { readdirSync, statSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { launch, q, sleep } from "./lib.mjs";
const [url] = process.argv.slice(2);
const out = resolve(process.env.OUT ?? ".");
const p = await launch({ autoplay: true });
await p.cdp("Page.setDownloadBehavior", { behavior: "allow", downloadPath: out });
await p.goto(url);
await sleep(3000);
await p.ev(`${q(".start")}?.click()`);
await sleep(1500);

const key = async (k, code = k) => {
  // text only for keys that type a character (Chrome doesn't send an
  // Escape or Enter keydown with one)
  await p.cdp("Input.dispatchKeyEvent", { type: "keyDown", key: k, code, ...(k.length === 1 ? { text: k } : {}) });
  await p.cdp("Input.dispatchKeyEvent", { type: "keyUp", key: k, code });
  await sleep(300);
};
const dial = () => p.ev(`${q(".freq")}?.textContent`);
const btn = (text) => `[...document.querySelectorAll("button")].find((b) => b.textContent.trim() === ${JSON.stringify(text)})`;
let seen = new Set(readdirSync(out));
const newFiles = async (wait = 3000) => {
  await sleep(wait);
  const now = readdirSync(out).filter((f) => !seen.has(f) && !f.endsWith(".crdownload"));
  for (const f of now) console.log("  saved:", f, statSync(`${out}/${f}`).size, "bytes");
  seen = new Set(readdirSync(out));
  return now;
};

console.log("== look");
await p.ev(`(() => { const s = ${q("select.palette")}; s.value = "heat"; s.dispatchEvent(new Event("change", { bubbles: true })); })()`);
await p.ev(`${btn("Peak")}.click()`);
await p.ev(`${btn("Time")}.click()`);
await sleep(6000);
console.log("  palette", await p.ev(`localStorage.getItem("web888.palette")`), "peak", await p.ev(`localStorage.getItem("web888.peak")`), "times", await p.ev(`localStorage.getItem("web888.wfTimes")`));
await p.shot(`${out}/listener-look.png`);
await p.ev(`${btn("PNG")}.click()`);
await newFiles();

console.log("== key help");
await key("?", "Slash");
console.log("  open:", await p.ev(`!!${q(".keyhelp")}`), "|", (await p.ev(`${q(".keyhelp")}?.textContent`))?.slice(0, 80));
await p.shot(`${out}/listener-keys.png`);
await key("Escape");
console.log("  closed:", !(await p.ev(`!!${q(".keyhelp")}`)));

console.log("== memories");
const d0 = await dial();
await key("m", "KeyM");
console.log("  popup:", await p.ev(`!!${q(".mem-pop")}`));
await p.type(q(".mem-store input"), "test memory");
await p.ev(`${q(".mem-store button")}.click()`);
await sleep(300);
console.log("  rows:", await p.ev(`[...document.querySelectorAll(".mem-list tr")].map((r) => r.textContent).join(" | ")`));
await p.ev(`${q(".mem-file button")}.click()`);
const exp = await newFiles(1500);
await key("Escape");
// elsewhere, then back from the memory
await p.ev(`${q(".freq")}.click()`);
await sleep(200);
await p.type(q(".freq-input"), "9500");
await key("Enter");
await sleep(800);
const d1 = await dial();
await key("m", "KeyM");
await p.ev(`${q(".mem-list tr")}.click()`);
await sleep(800);
console.log(`  dial ${d0} -> ${d1} -> recalled ${await dial()}`);
// import the export again with a second entry added
if (exp[0]) {
  const f = `${out}/import.json`;
  writeFileSync(f, JSON.stringify({ web888_memories: 1, memories: [{ hz: 10_000_000, mode: "am", lo: -4500, hi: 4500, name: "WWV" }] }));
  await key("m", "KeyM");
  const doc = await p.cdp("DOM.getDocument");
  const node = await p.cdp("DOM.querySelector", { nodeId: doc.result.root.nodeId, selector: ".mem-file input[type=file]" });
  await p.cdp("DOM.setFileInputFiles", { nodeId: node.result.nodeId, files: [f] });
  await sleep(500);
  console.log("  import:", await p.ev(`${q(".mem-file .dim")}?.textContent`), "| rows", await p.ev(`document.querySelectorAll(".mem-list tr").length`));
  await p.shot(`${out}/listener-mem.png`);
  await key("Escape");
}

console.log("== VFO A/B");
const a = await dial();
await key("v", "KeyV");
await p.ev(`${q(".freq")}.click()`);
await sleep(200);
await p.type(q(".freq-input"), "7100");
await key("Enter");
await sleep(800);
const b = await dial();
await key("v", "KeyV");
await sleep(800);
const back = await dial();
await key("v", "KeyV");
await sleep(800);
console.log(`  A ${a}, B set to ${b}, swap -> ${back}, swap -> ${await dial()} | ${await p.ev(`${q(".vfo button")}.textContent`)}`);

console.log("== audio recording");
await p.ev(`${btn("Rec ▾")}.click()`);
await sleep(200);
await p.ev(`${q('.iqrec-pop button[data-arec="60"]')}.click()`);
await sleep(5000);
console.log("  recording:", await p.ev(`${q("button.arec")}?.textContent`));
await p.ev(`${q("button.arec")}.click()`);
await newFiles();

console.log("== AGC custom");
await p.ev(`${btn("C")}?.click()`);
await sleep(800);
const labels = await p.ev(`[...document.querySelectorAll("footer label")].map((l) => l.textContent.trim()).join(" | ")`);
console.log("  ", labels);
await p.shot(`${out}/listener-footer.png`);
p.close();
