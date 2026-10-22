// SPDX-License-Identifier: GPL-3.0-or-later
// Reading the config's JSON Schema (as schemars writes it) for the form:
// which editor a field gets, and parsing its text back.

import type { Json } from "./api.ts";

export interface Schema {
  $ref?: string;
  $defs?: Record<string, Schema>;
  type?: string | string[];
  description?: string;
  properties?: Record<string, Schema>;
  oneOf?: Schema[];
  enum?: Json[];
  const?: Json;
  default?: Json;
  format?: string;
  minimum?: number;
  maximum?: number;
}

/** `s` with a `$ref` replaced by its definition (keeping the field's own
 * description and default). */
export function resolve(s: Schema, root: Schema): Schema {
  if (!s.$ref) return s;
  const name = s.$ref.replace(/^#\/\$defs\//, "");
  const d = root.$defs?.[name] ?? {};
  return { ...d, description: s.description ?? d.description, default: s.default ?? d.default };
}

export type Kind =
  | { kind: "object"; fields: [string, Schema][] }
  | { kind: "choice"; options: { value: string; help?: string }[]; nullable: boolean }
  | { kind: "bool" }
  | { kind: "number"; integer: boolean; nullable: boolean }
  | { kind: "text"; nullable: boolean }
  | { kind: "json" };

function types(s: Schema): string[] {
  return s.type === undefined ? [] : Array.isArray(s.type) ? s.type : [s.type];
}

/** The editor for (resolved) schema `s`. */
export function kindOf(s: Schema): Kind {
  const t = types(s);
  const nullable = t.includes("null");
  if (s.properties) return { kind: "object", fields: Object.entries(s.properties) };
  const consts = s.oneOf?.every((o) => typeof o.const === "string");
  if (s.oneOf && consts) {
    return { kind: "choice", options: s.oneOf.map((o) => ({ value: o.const as string, help: o.description })), nullable };
  }
  if (s.enum?.every((v) => typeof v === "string" || v === null)) {
    return {
      kind: "choice",
      options: s.enum.filter((v): v is string => v !== null).map((v) => ({ value: v })),
      nullable: nullable || s.enum.includes(null),
    };
  }
  const base = t.filter((x) => x !== "null");
  if (base.length === 1) {
    if (base[0] === "boolean") return { kind: "bool" };
    if (base[0] === "integer" || base[0] === "number") return { kind: "number", integer: base[0] === "integer", nullable };
    if (base[0] === "string") return { kind: "text", nullable };
  }
  return { kind: "json" };
}

/** Text of a scalar for an input. */
export function show(v: Json | undefined): string {
  if (v === null || v === undefined) return "";
  return typeof v === "object" ? JSON.stringify(v) : String(v);
}

/** Parse an input's text for `k`; an error message instead of a value. */
export function parse(k: Kind, text: string): { value: Json } | { error: string } {
  const t = text.trim();
  switch (k.kind) {
    case "number": {
      if (t === "") return k.nullable ? { value: null } : { error: "a number is needed" };
      const x = Number(t);
      if (!Number.isFinite(x)) return { error: "not a number" };
      if (k.integer && (!Number.isInteger(x) || x < 0)) return { error: "a whole number is needed" };
      return { value: x };
    }
    case "text":
      return t === "" && k.nullable ? { value: null } : { value: text };
    case "json":
      try {
        return { value: JSON.parse(t) as Json };
      } catch (e) {
        return { error: (e as Error).message };
      }
    default:
      return { error: "not a text field" };
  }
}

/** Equal as JSON (key order ignored). */
export function same(a: Json | undefined, b: Json | undefined): boolean {
  if (a === b) return true;
  if (a === null || b === null || typeof a !== "object" || typeof b !== "object") return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  const ka = Object.keys(a);
  const kb = Object.keys(b);
  if (ka.length !== kb.length) return false;
  return ka.every((k) => same((a as Record<string, Json>)[k], (b as Record<string, Json>)[k]));
}

/** `obj` with the value at `path` replaced (copies along the path). */
export function setIn(obj: Json, path: string[], v: Json): Json {
  if (path.length === 0) return v;
  const o = (obj && typeof obj === "object" && !Array.isArray(obj) ? obj : {}) as Record<string, Json>;
  const [k, ...rest] = path;
  return { ...o, [k]: setIn(o[k] ?? null, rest, v) };
}

export function getIn(obj: Json | undefined, path: string[]): Json | undefined {
  let o = obj;
  for (const k of path) {
    if (!o || typeof o !== "object" || Array.isArray(o)) return undefined;
    o = o[k];
  }
  return o;
}

/** "wf_budget" -> "WF budget" */
export function label(key: string): string {
  const words = key.split("_").map((w) => (["wf", "ip", "tls", "iq", "adc", "ui", "hpsdr"].includes(w) ? w.toUpperCase() : w));
  const s = words.join(" ");
  return s.charAt(0).toUpperCase() + s.slice(1);
}
