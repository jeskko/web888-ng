// SPDX-License-Identifier: GPL-3.0-or-later
// Accessibility checks of the receiver page: the accessibility tree
// (controls without a name), text contrast (WCAG AA), live regions, the
// Tab order with a visible focus ring; then keyboard flows: key g and a
// typed frequency, toggle states, dialogs (focus in, kept in, shortcuts
// quiet, Escape, focus back), the key help, a drop-down's arrow keys,
// rows reachable from the keyboard. Screenshot to OUT.
//   CHROME=... OUT=/tmp/x node a11y.mjs URL [label width height]
import { launch, sleep } from "./lib.mjs";
const [url, label = "page", w = "1400", h = "900"] = process.argv.slice(2);
const p = await launch({ width: +w, height: +h });
await p.goto(url); await sleep(3500);
if (process.env.PRE) { await p.ev(process.env.PRE); await sleep(600); }
await p.cdp("Accessibility.enable");
const ax = (await p.cdp("Accessibility.getFullAXTree")).result.nodes;
const roles = {}; const unnamed = [];
const inter = new Set(["button","link","textbox","combobox","slider","checkbox","menuitem","spinbutton","radio","switch","tab","listbox","searchbox","menuitemcheckbox"]);
for (const n of ax) { if (n.ignored) continue; const r = n.role?.value; roles[r] = (roles[r] || 0) + 1;
  if (inter.has(r) && !(n.name?.value || "").trim()) unnamed.push(r); }
console.log(`== ${label}: AX roles`, JSON.stringify(Object.fromEntries(Object.entries(roles).sort((a,b)=>b[1]-a[1]))));
const uc = {}; unnamed.forEach(r => uc[r] = (uc[r]||0)+1); console.log("unnamed interactive:", JSON.stringify(uc));
const r = await p.ev(`(() => {
  const lum = (c) => { const m = c.match(/[\\d.]+/g).map(Number); const f = (v) => { v/=255; return v<=0.03928? v/12.92 : ((v+0.055)/1.055)**2.4; }; return [0.2126*f(m[0])+0.7152*f(m[1])+0.0722*f(m[2]), m[3] ?? 1]; };
  const bgOf = (e) => { for (; e; e = e.parentElement) { const b = getComputedStyle(e).backgroundColor; const m = b.match(/[\\d.]+/g); if (m && (m.length < 4 || +m[3] > 0.5)) return b; } return "rgb(255,255,255)"; };
  const fails = {}; let n = 0, bad = 0;
  const walk = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  for (let t; (t = walk.nextNode()); ) { const s = t.textContent.trim(); if (!s) continue; const e = t.parentElement; const cs = getComputedStyle(e);
    if (cs.visibility === "hidden" || cs.display === "none" || !e.offsetParent && cs.position !== "fixed") continue; const rc = e.getBoundingClientRect(); if (!rc.width || rc.bottom < 0 || rc.top > innerHeight) continue;
    n++; const [lf, a] = lum(cs.color); const [lb] = lum(bgOf(e)); const ratio = (Math.max(lf,lb)+0.05)/(Math.min(lf,lb)+0.05) * (a < 1 ? 1 : 1);
    const px = parseFloat(cs.fontSize), bold = +cs.fontWeight >= 700; const need = (px >= 24 || (bold && px >= 18.66)) ? 3 : 4.5;
    if (ratio < need || a < 0.6) { bad++; const k = cs.color + " on " + bgOf(e) + " " + px + "px"; (fails[k] ??= { ratio: +ratio.toFixed(2), n: 0, ex: [] }).n++; const f = fails[k]; if (f.ex.length < 3) f.ex.push(s.slice(0, 24)); } }
  const sizes = {}; document.querySelectorAll("body *").forEach((e) => { if (!e.childNodes.length || ![...e.childNodes].some(c => c.nodeType===3 && c.textContent.trim())) return; const px = Math.round(parseFloat(getComputedStyle(e).fontSize)); sizes[px] = (sizes[px]||0)+1; });
  return { texts: n, failing: bad, fails, sizes, canvases: document.querySelectorAll("canvas").length, live: [...document.querySelectorAll("[aria-live],[role=status],[role=log],[role=alert]")].map(e=>e.className||e.tagName), lang: document.documentElement.lang, viewport: document.querySelector("meta[name=viewport]")?.content };
})()`);
console.log(`text nodes ${r.texts}, below AA ${r.failing}; canvases ${r.canvases}; live regions ${JSON.stringify(r.live)}; lang=${r.lang}; viewport=${r.viewport}`);
console.log("font px histogram", JSON.stringify(r.sizes));
for (const [k, v] of Object.entries(r.fails).sort((a,b)=>b[1].n-a[1].n).slice(0, 14)) console.log(`  ${v.ratio}:1  x${v.n}  ${k}  e.g. ${JSON.stringify(v.ex)}`);
// keyboard traversal
await p.ev("document.activeElement?.blur(); document.body.focus()");
const seen = []; let novis = 0;
for (let i = 0; i < 80; i++) { await p.cdp("Input.dispatchKeyEvent", { type: "keyDown", key: "Tab", code: "Tab", windowsVirtualKeyCode: 9 }); await p.cdp("Input.dispatchKeyEvent", { type: "keyUp", key: "Tab", code: "Tab", windowsVirtualKeyCode: 9 });
  const d = await p.ev(`(() => { const e = document.activeElement; if (!e || e === document.body) return null; const cs = getComputedStyle(e); const vis = (cs.outlineStyle !== "none" && parseFloat(cs.outlineWidth) > 0) || cs.boxShadow !== "none"; return { d: e.tagName.toLowerCase() + (e.className ? "." + String(e.className).split(" ")[0] : "") + ":" + (e.getAttribute("aria-label") || e.title || e.textContent || e.value || "").trim().slice(0, 18), vis }; })()`);
  if (!d) { seen.push("(body)"); continue; } if (!d.vis) novis++; seen.push(d.d + (d.vis ? "" : " [no ring]")); if (seen.filter(s => s === seen.at(-1)).length > 1) break; }
console.log(`tab stops (${seen.length}, ${novis} without a visible focus ring):`); console.log("  " + seen.join(" | "));
p.close();
await sleep(800);
{
const p = await launch({ width: 1400, height: 900 });
await p.goto(url); await sleep(3500);
const key = async (k, code = k, vk = 0, text) => { await p.cdp("Input.dispatchKeyEvent", { type: "keyDown", key: k, code, windowsVirtualKeyCode: vk, ...(text ? { text } : {}) }); await p.cdp("Input.dispatchKeyEvent", { type: "keyUp", key: k, code, windowsVirtualKeyCode: vk }); await sleep(250); };
const act = () => p.ev(`(() => { const e = document.activeElement; return e ? (e.tagName + "." + (e.className||"") + " " + (e.getAttribute("aria-label")||e.title||e.textContent||"").trim().slice(0,40)) : null; })()`);
await p.cdp("Accessibility.enable");
const ax = (await p.cdp("Accessibility.getFullAXTree")).result.nodes.filter(n => !n.ignored);
const pressed = ax.filter(n => n.properties?.some(q => q.name === "pressed"));
console.log("buttons with a pressed state:", pressed.length, "pressed now:", pressed.filter(n => n.properties.find(q=>q.name==="pressed").value.value === "true").map(n => n.name.value.slice(0,20)).join(" | "));
// g opens the frequency entry; type 7030 Enter
await p.ev("document.body.focus()"); await key("g", "KeyG", 71, "g");
console.log("after g, focus:", await act());
await p.cdp("Input.insertText", { text: "7030" }); await key("Enter", "Enter", 13, "\r");
console.log("freq now:", await p.ev("document.querySelector('.freq').getAttribute('aria-label')"));
// dialogs: focus moves in, Tab stays in, Escape closes and focus returns
for (const [name, sel] of [["Config", "header .tools .group:last-child button"], ["Info", ".status button.link[aria-haspopup]"]]) {
  await p.ev(`document.querySelector(${JSON.stringify(sel)}).focus()`);
  await key("Enter", "Enter", 13, "\r"); await sleep(400);
  const inDlg = await p.ev(`!!document.activeElement?.closest("[role=dialog]")`);
  for (let i = 0; i < 40; i++) await p.cdp("Input.dispatchKeyEvent", { type: "keyDown", key: "Tab", code: "Tab", windowsVirtualKeyCode: 9 });
  const still = await p.ev(`!!document.activeElement?.closest("[role=dialog]")`);
  await key("u", "KeyU", 85, "u");
  const mode = await p.ev(`document.querySelector("header button[aria-pressed=true]")?.textContent`);
  await key("Escape", "Escape", 27);
  console.log(`${name}: focus inside ${inDlg}, after 40 Tabs still inside ${still}, "u" behind it left the mode ${mode}; Escape closed ${!(await p.ev('!!document.querySelector("[role=dialog]")'))}, focus back on: ${await act()}`);
}
// keys help: Tab doesn't close it, Escape does
await p.ev("document.body.focus()"); await key("?", "Slash", 191, "?"); await sleep(300);
await key("Tab", "Tab", 9);
console.log("keys help open after Tab:", await p.ev('!!document.querySelector(".keyhelp-box")'));
await key("Escape", "Escape", 27);
console.log("keys help open after Escape:", await p.ev('!!document.querySelector(".keyhelp-box")'));
// a drop-down: arrows move, Escape returns to the button
await p.ev(`document.querySelector("button.drop-btn").focus()`); await key("Enter", "Enter", 13, "\r"); await sleep(300);
const a1 = await act(); await key("ArrowDown", "ArrowDown", 40); const a2 = await act();
const hz1 = await p.ev("document.querySelector('.freq').getAttribute('aria-label')");
await key("Escape", "Escape", 27);
console.log(`drop: focus ${a1} -> ArrowDown -> ${a2}; dial unchanged: ${hz1}; after Escape focus: ${await act()}`);
// a spot row from the keyboard: open spots, Tab to a row, Enter
await p.ev(`[...document.querySelectorAll("header button")].find(b => /DX/.test(b.title))?.click()`); await sleep(1500);
const rows = await p.ev(`document.querySelectorAll(".spots-list tr[tabindex]").length`);
console.log("spot rows reachable:", rows);
// OVL lamp text and the near-dial list
console.log("lamp:", await p.ev(`document.querySelector(".lamp:not(.att)")?.textContent`));
console.log("live regions:", await p.ev(`[...document.querySelectorAll("[aria-live],[role=status],[role=alert]")].map(e => e.getAttribute("role")||"live").join(",")`));
await p.shot(`${process.env.OUT ?? "."}/a11y-desktop.png`);
p.close();
}
process.exit(0);
