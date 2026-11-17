// SPDX-License-Identifier: GPL-3.0-or-later
// Frequency memories and the second VFO, kept in this browser (pure,
// unit-tested; the signals are in state.ts).
//
// A memory is a frequency with its mode and passband and an optional
// name. The list is sorted by frequency; storing at a frequency and mode
// already there replaces that one. Export/import is a JSON file:
// {"web888_memories": 1, "memories": [{hz, mode, lo, hi, name}]}.

import { MODES, type Mode } from "./proto.ts";

export interface Tuning {
  hz: number;
  mode: Mode;
  lo: number;
  hi: number;
}

export interface Memory extends Tuning {
  name: string;
}

/** Most memories kept. */
export const MEM_MAX = 500;
/** Longest memory name (characters). */
export const MEM_NAME_MAX = 24;

/** The tuning, if `v` is one (frequency 0-2 GHz, a known mode, a
 * passband of at most ±20 kHz with lo < hi); else null. */
export function asTuning(v: unknown): Tuning | null {
  if (!v || typeof v !== "object") return null;
  const o = v as Record<string, unknown>;
  const num = (x: unknown) => (typeof x === "number" && Number.isFinite(x) ? x : null);
  const hz = num(o.hz);
  const lo = num(o.lo);
  const hi = num(o.hi);
  const mode = o.mode;
  if (hz === null || lo === null || hi === null || typeof mode !== "string") return null;
  if (!(MODES as string[]).includes(mode)) return null;
  if (hz < 0 || hz > 2e9 || lo >= hi || lo < -20_000 || hi > 20_000) return null;
  return { hz: Math.round(hz), mode: mode as Mode, lo: Math.round(lo), hi: Math.round(hi) };
}

/** A memory from untrusted data, or null. */
export function asMemory(v: unknown): Memory | null {
  const t = asTuning(v);
  if (!t) return null;
  const n = (v as Record<string, unknown>).name;
  return { ...t, name: cleanMemName(typeof n === "string" ? n : "") };
}

export function cleanMemName(s: string): string {
  return [...s.trim().replace(/\s+/g, " ").replace(/\p{Cc}/gu, "")].slice(0, MEM_NAME_MAX).join("");
}

/** The list with `m` stored (replacing one at the same frequency and
 * mode), sorted by frequency, at most MEM_MAX (the new one stays). */
export function store(list: Memory[], m: Memory): Memory[] {
  const rest = list.filter((x) => !(x.hz === m.hz && x.mode === m.mode));
  const out = [...rest, m].sort((a, b) => a.hz - b.hz || a.mode.localeCompare(b.mode));
  while (out.length > MEM_MAX) out.splice(out[0] === m ? 1 : 0, 1);
  return out;
}

export function remove(list: Memory[], m: Memory): Memory[] {
  return list.filter((x) => !(x.hz === m.hz && x.mode === m.mode));
}

/** The export file's text. */
export function exportText(list: Memory[]): string {
  return JSON.stringify({ web888_memories: 1, memories: list }, null, 1);
}

/** Memories from a file (ours, or a bare array of them); bad entries are
 * skipped. Throws if the text isn't such a file. */
export function parseMemories(text: string): Memory[] {
  const v: unknown = JSON.parse(text);
  const arr = Array.isArray(v) ? v : v && typeof v === "object" && Array.isArray((v as { memories?: unknown }).memories) ? (v as { memories: unknown[] }).memories : null;
  if (!arr) throw new Error("not a memories file");
  return arr.map(asMemory).filter((m): m is Memory => m !== null);
}

/** `list` with every memory of `more` stored. */
export function merge(list: Memory[], more: Memory[]): Memory[] {
  return more.reduce(store, list);
}

/** The memory nearest `hz` within `tol` Hz in `list` (sorted), or null:
 * the dial's memory name. */
export function memoryAt(list: Memory[], hz: number, mode: Mode, tol = 10): Memory | null {
  return list.find((m) => m.mode === mode && Math.abs(m.hz - hz) <= tol) ?? null;
}
