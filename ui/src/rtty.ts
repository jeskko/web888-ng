// SPDX-License-Identifier: GPL-3.0-or-later
// FSK / RTTY decoding in the browser (pure parts, unit-tested): stations
// to tune to, the skimmer's parameters in RF terms (the sideband decides
// which RF tone is mark), and its output (rtty/worker.ts, the web888-fsk
// crate in WebAssembly) merged into one entry per signal.

import type { Mode } from "./proto.ts";
import { N_, t } from "./i18n.ts";

/** Parameters as the worker reports them (tones in audio Hz). */
export interface FskP {
  baud: number;
  shift: number;
  lo: number;
  hi: number;
  /** Mark is the upper audio tone. */
  markHigh: boolean;
  framing: string;
  charset: string;
  usos: boolean;
  msb: boolean;
  mode: "async" | "sitor" | "dsc" | "sync";
}

export type FskEv =
  | { k: "lock"; id: number; hz: number; p: FskP }
  | { k: "text"; id: number; hz: number; t: number; text: string; weak?: string }
  | { k: "ident"; hz: number; p: FskP };

/** What the worker posts. */
export interface FskRun {
  ev: FskEv[];
  act: { id: number; hz: number; locked: boolean; snr: number; fe: number; p: FskP }[];
  now: number;
}

/** A signal as listed. */
export interface FskSignal {
  /** 0: identified, not decoded. */
  id: number;
  /** RF frequency of the centre between the tones (Hz). */
  rf: number;
  p: FskP;
  snr: number;
  fe: number;
  text: string;
  /** Per character of `text`, "1" where it read weakly (a bit near the
   * threshold or a framing error: it may have read otherwise). */
  weak: string;
  locked: boolean;
  /** Still in the skimmer's list. */
  active: boolean;
  /** UTC seconds of the last change. */
  last: number;
}

/** Text kept per signal (characters). */
export const RTTY_TEXT = 4000;
/** A signal gone from the skimmer stays listed this long (s). */
export const RTTY_LINGER = 300;

/** Stations to tune to: the centre between the tones (Hz). */
export const RTTY_STATIONS: { name: string; hz: number }[] = [
  { name: "NAVTEX 518 kHz", hz: 518_000 },
  { name: "NAVTEX 490 kHz", hz: 490_000 },
  { name: "DWD 4583 kHz", hz: 4_583_000 },
  { name: "DWD 7646 kHz", hz: 7_646_000 },
  { name: "DWD 10100.8 kHz", hz: 10_100_800 },
  { name: "DWD 11039 kHz", hz: 11_039_000 },
  { name: "DWD 14467.3 kHz", hz: 14_467_300 },
  { name: "DSC 2187.5 kHz", hz: 2_187_500 },
  { name: "DSC 4207.5 kHz", hz: 4_207_500 },
  { name: "DSC 6312 kHz", hz: 6_312_000 },
  { name: "DSC 8414.5 kHz", hz: 8_414_500 },
  { name: "DSC 12577 kHz", hz: 12_577_000 },
  { name: "DSC 16804.5 kHz", hz: 16_804_500 },
  { name: "20 m RTTY 14.080 MHz", hz: 14_081_000 },
  { name: "40 m RTTY 7.040 MHz", hz: 7_041_000 },
];

/** Audio frequency the station's centre is tuned to (USB). */
export const RTTY_AUDIO = 1000;

/** RF of an audio frequency in this mode (CW: its pitch at the dial). */
export function rfOf(mode: Mode, dial: number, pitch: number, hz: number): number {
  if (mode === "lsb") return dial - hz;
  if (mode === "cw") return dial + hz - pitch;
  return dial + hz;
}

/** One line about a signal's parameters, in RF terms. */
export function describe(p: FskP, mode: Mode): string {
  const parts = [`${p.baud} Bd`, `${Math.round(p.shift)} Hz`];
  if (p.mode === "sitor") parts.push("SITOR-B");
  else if (p.mode === "dsc") parts.push("DSC");
  else if (p.mode === "async") parts.push(p.framing);
  // LSB mirrors: the upper audio tone is the lower RF one
  const upper = mode === "lsb" ? !p.markHigh : p.markHigh;
  // i18n: mark = the FSK tone for a 1 bit; upper/lower = in frequency, relative to the space tone
  if (p.mode !== "sync" && p.mode !== "dsc") parts.push(upper ? t("mark upper") : t("mark lower"));
  if (p.mode === "dsc") return parts.join(" · ");
  let cs = p.charset;
  // i18n: MSB first = most significant bit sent first; USOS = unshift on space (ITA2 teleprinter case-shift convention)
  if (p.msb) cs = t("{charset} MSB first", { charset: cs });
  if (cs === "ITA2" || cs === "US-TTY") cs = p.usos ? cs : t("{charset}, no USOS", { charset: cs });
  parts.push(cs);
  return parts.join(" · ");
}

/** The signals after a run. `rf` maps an audio frequency to RF; `now`
 * is UTC seconds. */
export function applyFsk(prev: FskSignal[], run: FskRun, now: number, rf: (hz: number) => number): FskSignal[] {
  const by = new Map<string, FskSignal>(prev.map((s) => [key(s), { ...s }]));
  const get = (id: number, hz: number, p: FskP): FskSignal => {
    const k = id ? `#${id}` : `@${Math.round(rf(hz) / 100)}`;
    let s = by.get(k);
    if (!s) {
      s = { id, rf: rf(hz), p, snr: -99, fe: 0, text: "", weak: "", locked: id !== 0, active: true, last: now };
      by.set(k, s);
    }
    return s;
  };
  for (const e of run.ev) {
    if (e.k === "lock") Object.assign(get(e.id, e.hz, e.p), { p: e.p, rf: rf(e.hz), locked: true, last: now });
    else if (e.k === "ident") Object.assign(get(0, e.hz, e.p), { p: e.p, last: now });
    else {
      const s = by.get(`#${e.id}`);
      if (!s) continue;
      let text = s.text + e.text;
      let weak = s.weak + (e.weak?.length === e.text.length ? e.weak : "0".repeat(e.text.length));
      if (text.length > RTTY_TEXT) {
        text = text.slice(text.length - RTTY_TEXT);
        weak = weak.slice(weak.length - RTTY_TEXT);
      }
      Object.assign(s, { text, weak, last: now });
    }
  }
  for (const s of by.values()) s.active = false;
  for (const a of run.act) {
    const s = get(a.id, a.hz, a.p);
    Object.assign(s, { rf: rf(a.hz), p: a.p, snr: a.snr, fe: a.fe, locked: a.locked, active: true });
  }
  return [...by.values()].filter((s) => s.active || now - s.last < RTTY_LINGER).sort((a, b) => a.rf - b.rf);
}

/** The last `n` characters of a signal's text as runs read weakly or
 * not (a leading "…" when cut). */
export function textRuns(text: string, weak: string, n: number): { s: string; weak: boolean }[] {
  const cut = text.length > n;
  const txt = cut ? text.slice(text.length - n) : text;
  const w = cut ? weak.slice(weak.length - n) : weak;
  const out: { s: string; weak: boolean }[] = cut ? [{ s: "…", weak: false }] : [];
  for (let i = 0; i < txt.length; ) {
    const k = w[i] === "1";
    let j = i + 1;
    while (j < txt.length && (w[j] === "1") === k) j++;
    const last = out[out.length - 1];
    if (last && last.weak === k) last.s += txt.slice(i, j);
    else out.push({ s: txt.slice(i, j), weak: k });
    i = j;
  }
  return out;
}

function key(s: FskSignal): string {
  return s.id ? `#${s.id}` : `@${Math.round(s.rf / 100)}`;
}

/** NAVTEX subjects (B2). */
// i18n: NAVTEX message subject classes (the B2 character of the message header)
export const NAVTEX_SUBJECTS: Record<string, string> = {
  A: N_("navigational warning"),
  B: N_("meteorological warning"),
  C: N_("ice report"),
  D: N_("search and rescue"),
  E: N_("weather forecast"),
  F: N_("pilot service"),
  G: "AIS",
  H: "LORAN",
  J: "GNSS",
  K: N_("other navaid"),
  L: N_("navigational warning"),
  T: N_("test"),
  V: N_("special service"),
  W: N_("special service"),
  X: N_("special service"),
  Y: N_("special service"),
  Z: N_("no message on hand"),
};

/** A NAVTEX message. */
export interface NavtexMsg {
  /** B1: the transmitter's letter. */
  station: string;
  /** B2: the subject letter. */
  subject: string;
  /** B3B4: the serial number. */
  number: string;
  text: string;
}

/** Complete NAVTEX messages (ZCZC B1B2B3B4 ... NNNN) in decoded text. */
export function navtex(text: string): NavtexMsg[] {
  const out: NavtexMsg[] = [];
  const re = /ZCZC ?([A-Z])([A-Z])(\d\d)\s*([\s\S]*?)NNNN/g;
  for (let m = re.exec(text); m; m = re.exec(text)) {
    out.push({ station: m[1], subject: m[2], number: m[3], text: m[4].replace(/\r/g, "").trim() });
  }
  return out;
}
