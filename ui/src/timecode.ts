// SPDX-License-Identifier: GPL-3.0-or-later
// Time-code stations decoded in the browser (pure parts, unit-tested): the
// stations, which one the tuning hears and where its tone is in the audio,
// and the receiver's output (timecode/worker.ts, the web888-timecode crate
// in WebAssembly) kept as the current minute's seconds and the minutes read.

import { AM_MODES, type Mode } from "./proto.ts";
import { N_, t } from "./i18n.ts";

export interface TcStation {
  /** The crate's index (tc_start). */
  id: number;
  name: string;
  /** Carriers (Hz). */
  carriers: number[];
  /** Heard as a keyed carrier (a tone in USB/LSB/CW), or WWV's 100 Hz
   * subcarrier (AM). */
  am: boolean;
  /** What it sends, for the panel. */
  about: string;
}

// i18n: about = where the time-code station is and how its time signal is keyed (carrier reduced/off for 0.1-0.8 s per second: a 0 or 1 bit, a marker)
export const TC_STATIONS: TcStation[] = [
  { id: 0, name: "DCF77", carriers: [77_500], am: false, about: N_("Mainflingen, Germany: carrier reduced 0.1 s (0) or 0.2 s (1) each second, none at second 59; CET/CEST") },
  { id: 1, name: "MSF", carriers: [60_000], am: false, about: N_("Anthorn, UK: carrier off 0.1 s, then bits A and B; 0.5 s at the minute; GMT/BST") },
  { id: 2, name: "WWVB", carriers: [60_000], am: false, about: N_("Fort Collins, USA: carrier reduced 0.2 s (0), 0.5 s (1), 0.8 s (marker); UTC") },
  { id: 3, name: "JJY", carriers: [40_000, 60_000], am: false, about: N_("Japan: carrier on 0.8 s (0), 0.5 s (1), 0.2 s (marker); JST") },
  { id: 4, name: "WWV/WWVH", carriers: [2_500_000, 5_000_000, 10_000_000, 15_000_000, 20_000_000, 25_000_000], am: true, about: N_("Fort Collins and Hawaii: the 100 Hz subcarrier 0.17 s (0), 0.47 s (1), 0.77 s (marker), heard in AM; UTC") },
];

/** WWV's subcarrier (Hz in AM audio). */
export const WWV_TONE = 100;

/** Audio frequency of a carrier at RF `rf` (USB, LSB, CW; null in AM and
 * the rest). */
export function audioOf(mode: Mode, dial: number, pitch: number, rf: number): number | null {
  if (mode === "usb") return rf - dial;
  if (mode === "lsb") return dial - rf;
  if (mode === "cw") return rf - dial + pitch;
  return null;
}

/** The station the tuning hears, and its tone in the audio: `want` a
 * station's name, or "auto" (the first whose carrier is in the passband;
 * at 60 kHz MSF before WWVB). An error says what to change. */
export function tcPlan(
  want: string,
  mode: Mode,
  dial: number,
  lo: number,
  hi: number,
  pitch: number,
): { station: TcStation; tone: number; carrier: number } | { error: string } {
  const list = want === "auto" ? TC_STATIONS : TC_STATIONS.filter((s) => s.name === want);
  const am = AM_MODES.includes(mode);
  for (const s of list) {
    for (const c of s.carriers) {
      if (s.am) {
        if (am && Math.abs(dial - c) <= 3000) return { station: s, tone: WWV_TONE, carrier: c };
        continue;
      }
      const a = audioOf(mode, dial, pitch, c);
      // in the passband (as audio: LSB's mirrored, CW's at its pitch), clear of its edges
      const [blo, bhi] = mode === "lsb" ? [-hi, -lo] : mode === "cw" ? [lo + pitch, hi + pitch] : [lo, hi];
      if (a !== null && a >= Math.max(blo, 0) + 50 && a <= bhi - 50 && a >= 100) return { station: s, tone: a, carrier: c };
    }
  }
  if (want !== "auto") {
    const s = list[0];
    if (!s) return { error: t("no station {name}", { name: want }) };
    // i18n: AM, USB, LSB, CW are mode names (kept in English); a station's carrier is its unmodulated radio frequency
    return {
      error: s.am
        ? t("{name}: tune AM to {mhz} MHz", { name: s.name, mhz: s.carriers.map((c) => c / 1e6).join(", ") })
        : t("{name}: tune its carrier ({khz} kHz) into the passband in USB, LSB or CW", { name: s.name, khz: s.carriers.map((c) => c / 1e3).join(" or ") }),
    };
  }
  return { error: t("No time station in the passband: pick one to tune to.") };
}

/** How to tune a station: USB with its carrier at 1 kHz in a narrow
 * passband, or AM on WWV. */
export function tcTuning(s: TcStation, carrier = s.carriers[0]): { dial: number; mode: Mode; lo: number; hi: number } {
  if (s.am) return { dial: carrier, mode: "am", lo: -5000, hi: 5000 };
  return { dial: carrier - 1000, mode: "usb", lo: 600, hi: 1400 };
}

/** What the worker posts. */
export interface TcRun {
  sec: { t: number; s: string; q: number; i: number }[];
  min: { t: number; utc: number; ok: boolean; confirmed: boolean; local: string; notes: string[]; sym: string }[];
  level: number;
  now: number;
}

/** A minute as listed. */
export interface TcMinute {
  station: string;
  /** UTC (Unix s) of its start, and the station's own words for it. */
  utc: number;
  local: string;
  ok: boolean;
  confirmed: boolean;
  notes: string[];
  /** When it was heard by the server's clock, less the time it says (s);
   * null without a clock. */
  late: number | null;
}

/** The minutes kept. */
export const TC_KEEP = 60;

/** The current minute's seconds: symbol and quality by second index;
 * before the minute's start is known, the last 60 as read. */
export function applySeconds(cur: { s: string; q: number }[], run: TcRun): { s: string; q: number }[] {
  let out = cur;
  for (const x of run.sec) {
    if (x.i < 0) {
      out = [...out, { s: x.s, q: x.q }].slice(-60);
      continue;
    }
    if (x.i === 0) out = [];
    else if (x.i < out.length) out = out.slice(0, x.i);
    else out = out.slice();
    while (out.length < x.i) out.push({ s: " ", q: 0 });
    out.push({ s: x.s, q: x.q });
  }
  return out;
}

/** UTC as "2026-11-16 10:24". */
export function utcText(utc: number): string {
  const d = new Date(utc * 1000);
  const p = (v: number) => String(v).padStart(2, "0");
  return `${d.getUTCFullYear()}-${p(d.getUTCMonth() + 1)}-${p(d.getUTCDate())} ${p(d.getUTCHours())}:${p(d.getUTCMinutes())}`;
}
