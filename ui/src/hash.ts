// SPDX-License-Identifier: GPL-3.0-or-later
// View state in the URL hash, so a link reopens the same frequency, mode
// and zoom: #f=7074.0&m=usb&z=256&c=7100.0 (kHz, mode, decimation, WF centre kHz).

import { MODES, type Mode } from "./proto.ts";

export interface HashState {
  hz?: number;
  mode?: Mode;
  decim?: number;
  centerHz?: number;
}

export function parseHash(h: string): HashState {
  const p = new URLSearchParams(h.replace(/^#/, ""));
  const num = (k: string) => {
    const v = p.get(k);
    if (v === null) return undefined;
    const x = Number(v);
    return Number.isFinite(x) ? x : undefined;
  };
  const m = p.get("m") as Mode | null;
  const f = num("f");
  const c = num("c");
  const z = num("z");
  return {
    hz: f === undefined ? undefined : f * 1e3,
    mode: m && MODES.includes(m) ? m : undefined,
    decim: z !== undefined && z >= 1 && Number.isInteger(Math.log2(z)) ? z : undefined,
    centerHz: c === undefined ? undefined : c * 1e3,
  };
}

export function formatHash(s: Required<HashState>): string {
  const k = (hz: number) => (hz / 1e3).toFixed(3).replace(/\.?0+$/, "");
  return `#f=${k(s.hz)}&m=${s.mode}&z=${s.decim}&c=${k(s.centerHz)}`;
}
