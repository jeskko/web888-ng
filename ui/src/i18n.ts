// SPDX-License-Identifier: GPL-3.0-or-later
// Translations of the web UI. The English text is the key (as in
// gettext): t("Start audio") is "Start audio" in English and the
// catalogue's msgstr in another language. Catalogues are ui/po/<lang>.po,
// compiled to dist/i18n/<lang>.json by `node scripts/i18n.mjs compile`
// (build.mjs runs it); only the chosen language's is fetched. See
// ui/po/README.md for how to translate.
//
// * t(msg, vars): placeholders in braces, t("{n} kbit/s", { n: 32 })
// * tc(ctx, msg, vars): the same words in another sense ("Peak" the hold
//   on the spectrum, "Peak" of the ADC): msgctxt in the catalogue
// * tn(one, many, n, vars): plural forms by the language's rule; {n} is n
// * tx(msg, parts): placeholders filled with elements (a link in a
//   sentence): returns nodes for JSX
// * N_(msg): marks text kept in a constant for extraction; translate it
//   where it's shown, t(item.label)
//
// Components that call these re-render when the language changes (they
// read the catalogue signal).

import { signal } from "@preact/signals";
import type { ComponentChildren } from "preact";

type Vars = Record<string, string | number>;
interface Catalogue {
  /** The PO header's Plural-Forms expression for n, e.g. "n != 1". */
  plural: string;
  /** key: msgid, or msgctxt + "\u0004" + msgid; value: msgstr (or the
   * plural forms in order). */
  messages: Record<string, string | string[]>;
}
export interface Language {
  code: string;
  /** In the language itself ("Suomi"). */
  name: string;
}

const STORE = "web888.lang";
const CTX = "\u0004";

/** The languages there are catalogues for (from dist/i18n/index.json),
 * English first. */
export const languages = signal<Language[]>([{ code: "en", name: "English" }]);
/** The language in use. */
export const lang = signal("en");
/** The listener's choice: "" follows the browser. */
export const langPref = signal(read());
const catalogue = signal<Catalogue | null>(null);
let pluralOf: (n: number) => number = (n) => (n === 1 ? 0 : 1);

function read(): string {
  try {
    return localStorage.getItem(STORE) ?? "";
  } catch {
    return "";
  }
}

function fill(s: string, vars?: Vars): string {
  if (!vars) return s;
  return s.replace(/\{(\w+)\}/g, (m, k: string) => (k in vars ? String(vars[k]) : m));
}

function look(key: string): string | string[] | undefined {
  return catalogue.value?.messages[key];
}

export function t(msg: string, vars?: Vars): string {
  const s = look(msg);
  return fill(typeof s === "string" && s ? s : msg, vars);
}

export function tc(ctx: string, msg: string, vars?: Vars): string {
  const s = look(ctx + CTX + msg);
  return fill(typeof s === "string" && s ? s : msg, vars);
}

export function tn(one: string, many: string, n: number, vars?: Vars): string {
  const v = { n, ...vars };
  const forms = look(one);
  if (Array.isArray(forms)) {
    const f = forms[pluralOf(n)];
    if (f) return fill(f, v);
  }
  return fill(n === 1 ? one : many, v);
}

/** Marks a constant's text for extraction; returns it unchanged. */
export function N_(msg: string): string {
  return msg;
}

/** A sentence with elements in it: t's text split at {name}, each filled
 * from `parts` (strings or elements). */
export function tx(msg: string, parts: Record<string, ComponentChildren>): ComponentChildren[] {
  const s = look(msg);
  const text = typeof s === "string" && s ? s : msg;
  const out: ComponentChildren[] = [];
  let last = 0;
  for (const m of text.matchAll(/\{(\w+)\}/g)) {
    out.push(text.slice(last, m.index));
    out.push(m[1] in parts ? parts[m[1]] : m[0]);
    last = m.index! + m[0].length;
  }
  out.push(text.slice(last));
  return out;
}

/** A Plural-Forms expression ("n != 1", "n%10==1 && n%100!=11 ? 0 : ...")
 * as a function: C's operators on integers, n the only name. */
export function pluralRule(expr: string): (n: number) => number {
  const tok = expr.match(/\d+|n|&&|\|\||[=!<>]=|[-+*/%<>!?:()]/g) ?? [];
  return (n: number) => {
    let i = 0;
    const peek = () => tok[i];
    const take = () => tok[i++];
    const prim = (): number => {
      const x = take();
      if (x === "(") {
        const v = cond();
        take();
        return v;
      }
      if (x === "!") return prim() ? 0 : 1;
      if (x === "-") return -prim();
      if (x === "n") return n;
      return Number(x);
    };
    const bin = (next: () => number, ops: string[], f: (op: string, a: number, b: number) => number) => (): number => {
      let a = next();
      while (ops.includes(peek())) {
        const op = take();
        a = f(op, a, next());
      }
      return a;
    };
    const mul = bin(prim, ["*", "/", "%"], (op, a, b) => (op === "*" ? a * b : op === "/" ? Math.trunc(a / b) : a % b));
    const add = bin(mul, ["+", "-"], (op, a, b) => (op === "+" ? a + b : a - b));
    const rel = bin(add, ["<", ">", "<=", ">="], (op, a, b) => Number(op === "<" ? a < b : op === ">" ? a > b : op === "<=" ? a <= b : a >= b));
    const eq = bin(rel, ["==", "!="], (op, a, b) => Number(op === "==" ? a === b : a !== b));
    const and = bin(eq, ["&&"], (_, a, b) => Number(a && b));
    const or = bin(and, ["||"], (_, a, b) => Number(a || b));
    const cond = (): number => {
      const c = or();
      if (peek() !== "?") return c;
      take();
      const a = cond();
      take();
      const b = cond();
      return c ? a : b;
    };
    return Math.trunc(cond());
  };
}

/** The best of the catalogues for the browser's languages ("fi-FI" ->
 * "fi"), else English. */
export function pick(wanted: readonly string[], have: readonly string[]): string {
  for (const w of wanted) {
    const l = w.toLowerCase();
    if (have.includes(l)) return l;
    const base = l.split("-")[0];
    if (have.includes(base)) return base;
  }
  return "en";
}

const base = () => new URL("i18n/", document.baseURI);

/** Loads the language list and the chosen catalogue (English: none). A
 * missing or broken file leaves the UI in English. */
export async function initI18n(): Promise<void> {
  try {
    const r = await fetch(new URL("index.json", base()));
    if (r.ok) {
      const list = (await r.json()) as Language[];
      languages.value = [{ code: "en", name: "English" }, ...list.filter((l) => l.code !== "en")];
    }
  } catch {
    // English only
  }
  await setLanguage(langPref.value, false);
}

/** "" follows the browser. */
export async function setLanguage(pref: string, remember = true): Promise<void> {
  if (remember) {
    langPref.value = pref;
    try {
      if (pref) localStorage.setItem(STORE, pref);
      else localStorage.removeItem(STORE);
    } catch {
      // not remembered
    }
  }
  const have = languages.value.map((l) => l.code);
  const code = pref && have.includes(pref) ? pref : pick(navigator.languages ?? [navigator.language], have);
  if (code === "en") {
    catalogue.value = null;
    pluralOf = (n) => (n === 1 ? 0 : 1);
    lang.value = "en";
    document.documentElement.lang = "en";
    return;
  }
  try {
    const r = await fetch(new URL(`${code}.json`, base()));
    if (!r.ok) throw new Error(String(r.status));
    const c = (await r.json()) as Catalogue;
    pluralOf = pluralRule(c.plural);
    catalogue.value = c;
    lang.value = code;
    document.documentElement.lang = code;
  } catch {
    catalogue.value = null;
    lang.value = "en";
  }
}

/** For tests: a catalogue without fetching. */
export function useCatalogue(c: Catalogue | null, code = "xx"): void {
  catalogue.value = c;
  pluralOf = c ? pluralRule(c.plural) : (n) => (n === 1 ? 0 : 1);
  lang.value = c ? code : "en";
}
