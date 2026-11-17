#!/usr/bin/env node
// SPDX-License-Identifier: GPL-3.0-or-later
// The web UI's translations (gettext PO files in ui/po/; see
// ui/po/README.md). No dependencies.
//
//   node scripts/i18n.mjs extract   src/ -> po/web888d-ui.pot
//   node scripts/i18n.mjs update    the template into every po/*.po (new
//                                   messages untranslated, gone ones kept
//                                   as #~ obsolete)
//   node scripts/i18n.mjs new xx "Name"   po/xx.po from the template
//   node scripts/i18n.mjs compile [out]   po/*.po -> out/i18n/*.json
//                                   (default dist), with index.json
//   node scripts/i18n.mjs check     CI: the template is current, every
//                                   translation keeps its placeholders
//   node scripts/i18n.mjs stats     how much of each language is done
//
// Messages are the string literals given to t(), tc(), tn(), tx() and
// N_() in src/ (see src/i18n.ts). A comment "// i18n: ..." on the line
// before (or at the end of the line) goes to translators as "#.".
import { mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import { join, relative } from "node:path";

const root = new URL("..", import.meta.url).pathname;
const SRC = join(root, "src");
const PO = join(root, "po");
const POT = join(PO, "web888d-ui.pot");
const CTX = "\u0004";

// ---------------------------------------------------------------- extract

async function files(dir) {
  const out = [];
  for (const e of await readdir(dir, { withFileTypes: true })) {
    const p = join(dir, e.name);
    if (e.isDirectory()) out.push(...(await files(p)));
    else if (/\.tsx?$/.test(e.name) && !e.name.endsWith(".d.ts") && e.name !== "i18n.ts") out.push(p);
  }
  return out.sort();
}

// a JS string literal at s[i] ("", '' or `` without ${}): [value, end] or null
function literal(s, i) {
  const q = s[i];
  if (q !== '"' && q !== "'" && q !== "`") return null;
  let v = "";
  for (let j = i + 1; j < s.length; j++) {
    const c = s[j];
    if (c === q) return [v, j + 1];
    if (q === "`" && c === "$" && s[j + 1] === "{") return null;
    if (c === "\\") {
      const d = s[++j];
      v += { n: "\n", t: "\t", r: "\r", "\\": "\\", '"': '"', "'": "'", "`": "`", u: null }[d] ?? d;
      if (d === "u") {
        const hex = s[j + 1] === "{" ? s.slice(j + 2, s.indexOf("}", j)) : s.slice(j + 1, j + 5);
        v = v.slice(0, -1) + String.fromCodePoint(parseInt(hex, 16));
        j += s[j + 1] === "{" ? hex.length + 2 : 4;
      }
      continue;
    }
    if (c === "\n" && q !== "`") return null;
    v += c;
  }
  return null;
}

const skip = (s, i) => {
  while (/\s/.test(s[i] ?? "")) i++;
  return i;
};

// comments blanked out (newlines kept, so positions and lines stay),
// strings and template literals skipped over
function stripComments(s) {
  let out = "";
  for (let i = 0; i < s.length; ) {
    const c = s[i];
    if (c === '"' || c === "'" || c === "`") {
      let j = i + 1;
      while (j < s.length && s[j] !== c) {
        if (s[j] === "\\") j++;
        else if (c !== "`" && s[j] === "\n") break;
        j++;
      }
      out += s.slice(i, j + 1);
      i = j + 1;
    } else if (c === "/" && s[i + 1] === "/") {
      const j = s.indexOf("\n", i);
      const end = j < 0 ? s.length : j;
      out += " ".repeat(end - i);
      i = end;
    } else if (c === "/" && s[i + 1] === "*") {
      const j = s.indexOf("*/", i + 2);
      const end = j < 0 ? s.length : j + 2;
      out += s.slice(i, end).replace(/[^\n]/g, " ");
      i = end;
    } else {
      out += c;
      i++;
    }
  }
  return out;
}

// a translators' note for the call on line `line` (1-based): "// i18n: ..."
// or "/* i18n: ... */" on that line or up to four lines before it, not
// past another call (whose note it would be)
const NOTE = /(?:\/\/|\/\*)\s*i18n:\s*(.*?)\s*(?:\*\/.*)?$/;
const CALL = /(?<![\w.$])(t|tc|tn|tx|N_)\(/;
function noteFor(lines, line) {
  const own = lines[line - 1]?.match(NOTE)?.[1];
  if (own) return own;
  for (let l = line - 2; l >= Math.max(0, line - 5); l--) {
    const text = lines[l] ?? "";
    const m = text.match(NOTE);
    if (m) return m[1];
    if (CALL.test(text.replace(/\/\/.*$/, ""))) return undefined;
  }
  return undefined;
}

/** The messages in one file, and calls whose text isn't a literal. */
function scan(source, file) {
  const found = [];
  const bad = [];
  const lines = source.split("\n");
  const text = stripComments(source);
  const lineOf = (i) => text.slice(0, i).split("\n").length;
  // an alias (import { t as tr }) hides the calls from this scan
  for (const m of text.matchAll(/import\s*\{([^}]*)\}\s*from\s*["'][^"']*i18n(\.ts)?["']/g))
    if (/\b(t|tc|tn|tx|N_)\s+as\s+\w+/.test(m[1])) {
      const l = lineOf(m.index);
      throw new Error(`${file}:${l}: import the translation functions under their own names (the extractor looks for t(, tc(, tn(, tx(, N_()`);
    }
  const re = /(?<![\w.$])(t|tc|tn|tx|N_)\(/g;
  for (const m of text.matchAll(re)) {
    const fn = m[1];
    let i = skip(text, m.index + m[0].length);
    const args = [];
    const want = fn === "tc" || fn === "tn" ? 2 : 1;
    let ok = true;
    for (let a = 0; a < want; a++) {
      const l = literal(text, i);
      if (!l) {
        ok = false;
        break;
      }
      args.push(l[0]);
      i = skip(text, l[1]);
      if (a < want - 1) {
        if (text[i] !== ",") {
          ok = false;
          break;
        }
        i = skip(text, i + 1);
      }
    }
    const line = lineOf(m.index);
    if (!ok) {
      // t(x) where x isn't a literal: fine for N_-marked constants shown
      // later (t(item.label)), so only reported, not an error
      bad.push(`${file}:${line}: ${lines[line - 1].trim().slice(0, 100)}`);
      continue;
    }
    const ctx = fn === "tc" ? args[0] : undefined;
    const msgid = fn === "tc" ? args[1] : args[0];
    const plural = fn === "tn" ? args[1] : undefined;
    const note = noteFor(lines, line);
    if (msgid) found.push({ msgid, plural, ctx, ref: `${file}:${line}`, note });
  }
  return { found, bad };
}

async function extract() {
  const all = new Map();
  const bad = [];
  for (const f of await files(SRC)) {
    const rel = relative(root, f);
    const { found, bad: b } = scan(await readFile(f, "utf8"), rel);
    bad.push(...b);
    for (const e of found) {
      const key = (e.ctx ? e.ctx + CTX : "") + e.msgid;
      const have = all.get(key);
      if (have) {
        have.refs.push(e.ref);
        if (e.note && !have.notes.includes(e.note)) have.notes.push(e.note);
        if (e.plural && !have.plural) have.plural = e.plural;
      } else all.set(key, { msgid: e.msgid, plural: e.plural, ctx: e.ctx, refs: [e.ref], notes: e.note ? [e.note] : [] });
    }
  }
  return { entries: [...all.values()], bad };
}

// ---------------------------------------------------------------- PO files

const esc = (s) => s.replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/\n/g, "\\n").replace(/\t/g, "\\t");
const unesc = (s) => s.replace(/\\(.)/g, (_, c) => ({ n: "\n", t: "\t", '"': '"', "\\": "\\" })[c] ?? c);

function parsePo(text) {
  const entries = [];
  let e = null;
  let field = null;
  const start = () => {
    if (e && (e.msgid !== undefined || e.obsolete)) entries.push(e);
    e = { notes: [], refs: [], flags: [], msgstr: [], prev: [] };
    field = null;
  };
  start();
  for (const raw of text.split("\n")) {
    const line = raw.trimEnd();
    if (line === "") {
      if (e.msgid !== undefined || e.obsolete) start();
      continue;
    }
    if (line.startsWith("#~")) {
      if (e.msgid !== undefined && !e.obsolete) start();
      e.obsolete = true;
      e.prev.push(line);
      continue;
    }
    if (line.startsWith("#.")) e.notes.push(line.slice(2).trim());
    else if (line.startsWith("#:")) e.refs.push(...line.slice(2).trim().split(/\s+/).filter(Boolean));
    else if (line.startsWith("#,")) e.flags.push(...line.slice(2).split(",").map((x) => x.trim()).filter(Boolean));
    else if (line.startsWith("#")) e.prev.push(line);
    else {
      const m = line.match(/^(msgctxt|msgid_plural|msgid|msgstr(?:\[(\d+)\])?)\s+"(.*)"$/);
      const cont = line.match(/^"(.*)"$/);
      if (m) {
        const v = unesc(m[3]);
        if (m[1] === "msgctxt") {
          if (e.msgid !== undefined) start();
          e.ctx = v;
          field = ["ctx"];
        } else if (m[1] === "msgid") {
          if (e.msgid !== undefined) start();
          e.msgid = v;
          field = ["msgid"];
        } else if (m[1] === "msgid_plural") {
          e.plural = v;
          field = ["plural"];
        } else {
          const k = m[2] === undefined ? 0 : Number(m[2]);
          e.msgstr[k] = v;
          field = ["msgstr", k];
        }
      } else if (cont && field) {
        const v = unesc(cont[1]);
        if (field[0] === "msgstr") e.msgstr[field[1]] += v;
        else e[field[0]] += v;
      }
    }
  }
  start();
  return entries;
}

const header = (po) => {
  const h = po.find((e) => e.msgid === "" && !e.ctx);
  const out = {};
  for (const l of (h?.msgstr[0] ?? "").split("\n")) {
    const i = l.indexOf(":");
    if (i > 0) out[l.slice(0, i).trim()] = l.slice(i + 1).trim();
  }
  return out;
};

function str(key, v) {
  if (!v.includes("\\n") || v.length < 70) return `${key} "${v}"`;
  // long text with line breaks: one PO line per line of text
  const parts = v.split(/(?<=\\n)/);
  return `${key} ""\n${parts.map((p) => `"${p}"`).join("\n")}`;
}

function writePo(entries, head) {
  const out = [];
  if (head) out.push(head.trimEnd(), "");
  for (const e of entries) {
    if (e.obsolete) {
      out.push(...e.prev, "");
      continue;
    }
    for (const n of e.notes) out.push(`#. ${n}`);
    if (e.refs.length) out.push(`#: ${e.refs.join(" ")}`);
    if (e.flags?.length) out.push(`#, ${e.flags.join(", ")}`);
    if (e.ctx !== undefined) out.push(str("msgctxt", esc(e.ctx)));
    out.push(str("msgid", esc(e.msgid)));
    if (e.plural !== undefined) {
      out.push(str("msgid_plural", esc(e.plural)));
      const n = Math.max(2, e.msgstr.length);
      for (let k = 0; k < n; k++) out.push(str(`msgstr[${k}]`, esc(e.msgstr[k] ?? "")));
    } else out.push(str("msgstr", esc(e.msgstr[0] ?? "")));
    out.push("");
  }
  return out.join("\n");
}

const POT_HEAD = `# The web888d web UI's messages (web888-ng, GPL-3.0-or-later).
# Made by \`node scripts/i18n.mjs extract\` from ui/src: don't edit, see
# po/README.md.
msgid ""
msgstr ""
"Project-Id-Version: web888d-ui\\n"
"MIME-Version: 1.0\\n"
"Content-Type: text/plain; charset=UTF-8\\n"
"Content-Transfer-Encoding: 8bit\\n"
`;

const potText = (entries) =>
  writePo(
    entries.map((e) => ({ ...e, flags: /\{\w+\}/.test(e.msgid) ? ["placeholders"] : [], msgstr: [] })),
    POT_HEAD,
  );

const key = (e) => (e.ctx !== undefined ? e.ctx + CTX : "") + e.msgid;

async function languages() {
  return (await readdir(PO)).filter((f) => /^[a-z]{2,3}(_[A-Z]{2})?\.po$/.test(f)).sort();
}

/** The template's messages merged into one language's file. */
function merge(potEntries, po) {
  const have = new Map(po.filter((e) => !e.obsolete && e.msgid !== "").map((e) => [key(e), e]));
  const out = [];
  for (const p of potEntries) {
    const old = have.get(key(p));
    have.delete(key(p));
    out.push({
      ...p,
      flags: old?.flags.filter((f) => f === "fuzzy") ?? [],
      msgstr: old ? old.msgstr : [],
      // translators' own comments ("# ...") stay with the message
      prev: old?.prev ?? [],
    });
  }
  // what is no longer in the source: kept, commented out, for reuse
  const gone = [...have.values()]
    .filter((e) => e.msgstr.some(Boolean))
    .map((e) => ({
      obsolete: true,
      prev: [...(e.ctx !== undefined ? [`#~ msgctxt "${esc(e.ctx)}"`] : []), `#~ msgid "${esc(e.msgid)}"`, ...e.msgstr.map((s, k) => (e.plural !== undefined ? `#~ msgstr[${k}] "${esc(s)}"` : `#~ msgstr "${esc(s)}"`))],
    }));
  const previous = po.filter((e) => e.obsolete);
  return writePo([...out, ...previous, ...gone]);
}

// the header's own lines, as written
function headerBlock(text) {
  const i = text.indexOf("\n\n");
  return i < 0 ? text : text.slice(0, i + 1);
}

// ---------------------------------------------------------------- commands

const placeholders = (s) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort().join(",");

async function cmdExtract() {
  const { entries, bad } = await extract();
  await writeFile(POT, potText(entries));
  console.log(`${relative(root, POT)}: ${entries.length} messages`);
  if (bad.length) console.log(`not literals (translated only if marked with N_ where defined):\n  ${bad.join("\n  ")}`);
}

async function cmdUpdate() {
  const pot = parsePo(await readFile(POT, "utf8")).filter((e) => e.msgid !== "");
  for (const f of await languages()) {
    const p = join(PO, f);
    const text = await readFile(p, "utf8");
    const po = parsePo(text);
    await writeFile(p, headerBlock(text) + "\n" + merge(pot, po));
    console.log(`${f}: ${stat(parsePo(await readFile(p, "utf8")))}`);
  }
}

async function cmdNew(code, name) {
  if (!code || !name) throw new Error('usage: new xx "Language name in that language"');
  const pot = parsePo(await readFile(POT, "utf8")).filter((e) => e.msgid !== "");
  const head = `# ${name} translation of the web888d web UI (web888-ng, GPL-3.0-or-later).
# See po/README.md.
msgid ""
msgstr ""
"Project-Id-Version: web888d-ui\\n"
"Language: ${code}\\n"
"X-Language-Name: ${name}\\n"
"MIME-Version: 1.0\\n"
"Content-Type: text/plain; charset=UTF-8\\n"
"Content-Transfer-Encoding: 8bit\\n"
"Plural-Forms: nplurals=2; plural=(n != 1);\\n"
`;
  await writeFile(join(PO, `${code}.po`), writePo(pot.map((e) => ({ ...e, flags: [], msgstr: [] })), head));
  console.log(`po/${code}.po: ${pot.length} messages to translate (check its Plural-Forms line)`);
}

function stat(po) {
  const ms = po.filter((e) => !e.obsolete && e.msgid !== "");
  const done = ms.filter((e) => e.msgstr.length && e.msgstr.every(Boolean) && !e.flags.includes("fuzzy")).length;
  const fuzzy = ms.filter((e) => e.flags.includes("fuzzy")).length;
  return `${done}/${ms.length} translated${fuzzy ? `, ${fuzzy} fuzzy` : ""}`;
}

async function cmdCompile(outDir = join(root, "dist")) {
  const dir = join(outDir, "i18n");
  await mkdir(dir, { recursive: true });
  const index = [];
  for (const f of await languages()) {
    const po = parsePo(await readFile(join(PO, f), "utf8"));
    const h = header(po);
    const code = f.replace(/\.po$/, "").toLowerCase().replace("_", "-");
    const plural = (h["Plural-Forms"] ?? "").match(/plural=\s*(.*?);?\s*$/)?.[1] ?? "n != 1";
    const messages = {};
    for (const e of po) {
      if (e.obsolete || e.msgid === "" || e.flags.includes("fuzzy")) continue;
      if (!e.msgstr.length || !e.msgstr.every(Boolean)) continue;
      messages[key(e)] = e.plural !== undefined ? e.msgstr : e.msgstr[0];
    }
    await writeFile(join(dir, `${code}.json`), JSON.stringify({ plural, messages }));
    index.push({ code, name: h["X-Language-Name"] ?? code });
  }
  await writeFile(join(dir, "index.json"), JSON.stringify(index));
  return index;
}

async function cmdCheck() {
  let fail = false;
  const { entries } = await extract();
  const want = potText(entries);
  const have = await readFile(POT, "utf8").catch(() => "");
  if (want !== have) {
    console.log("po/web888d-ui.pot is out of date: run `node scripts/i18n.mjs extract` and `update`");
    fail = true;
  }
  for (const f of await languages()) {
    const po = parsePo(await readFile(join(PO, f), "utf8"));
    const h = header(po);
    if (!h["Language"] || !h["X-Language-Name"] || !h["Plural-Forms"]) {
      console.log(`${f}: the header needs Language, X-Language-Name and Plural-Forms`);
      fail = true;
    }
    for (const e of po) {
      if (e.obsolete || e.msgid === "") continue;
      for (const [k, s] of e.msgstr.entries()) {
        if (!s) continue;
        const src = e.plural !== undefined && k > 0 ? e.plural : e.msgid;
        // a plural form may leave {n} out ("one" in some languages)
        const a = placeholders(src).split(",").filter((x) => x && !(e.plural !== undefined && x === "n"));
        const b = placeholders(s).split(",").filter((x) => x && !(e.plural !== undefined && x === "n"));
        if (a.join() !== b.join()) {
          console.log(`${f}: placeholders differ in "${e.msgid.slice(0, 60)}": {${a.join("}, {")}} vs {${b.join("}, {")}}`);
          fail = true;
        }
      }
    }
    console.log(`${f}: ${stat(po)}`);
  }
  if (fail) process.exit(1);
}

const [cmd, ...args] = process.argv.slice(2);
const cmds = { extract: cmdExtract, update: cmdUpdate, new: cmdNew, compile: async (o) => console.log(`i18n: ${(await cmdCompile(o)).map((l) => l.code).join(", ") || "no languages"}`), check: cmdCheck, stats: async () => {
  for (const f of await languages()) console.log(`${f}: ${stat(parsePo(await readFile(join(PO, f), "utf8")))}`);
} };
if (!cmds[cmd]) {
  console.log((await readFile(new URL(import.meta.url), "utf8")).split("\n").slice(2, 20).join("\n"));
  process.exit(2);
}
await cmds[cmd](...args);

