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
  /** Its key (the crate's and the server's name, a saved choice). */
  name: string;
  /** Shown instead of the key: today's callsign with the old name. */
  label?: string;
  /** Carriers (Hz). */
  carriers: number[];
  /** Heard as a keyed carrier (a tone in USB/LSB/CW), or WWV's 100 Hz
   * subcarrier (AM). */
  am: boolean;
  /** What it sends, for the panel. */
  about: string;
  /** Room its sidebands need around the carrier in the passband (Hz;
   * default 50: a keyed carrier). */
  margin?: number;
  /** AM: the tone keyed in the audio (Hz; default WWV's 100 Hz). */
  tone?: number;
  /** No time code: its minutes are named by the clock (BPM, RWM). */
  unnamed?: boolean;
}

// i18n: about = where the time-code station is and how its time signal is keyed (carrier reduced/off for 0.1-0.8 s per second: a 0 or 1 bit, a marker)
/** A station's name as shown. */
export function tcLabel(s: TcStation): string {
  return s.label ?? s.name;
}

export const TC_STATIONS: TcStation[] = [
  { id: 0, name: "DCF77", carriers: [77_500], am: false, about: N_("Mainflingen, Germany: carrier reduced 0.1 s (0) or 0.2 s (1) each second, none at second 59; CET/CEST") },
  { id: 1, name: "MSF", carriers: [60_000], am: false, about: N_("Anthorn, UK: carrier off 0.1 s, then bits A and B; 0.5 s at the minute; GMT/BST") },
  { id: 2, name: "WWVB", carriers: [60_000], am: false, about: N_("Fort Collins, USA: carrier reduced 0.2 s (0), 0.5 s (1), 0.8 s (marker); UTC") },
  { id: 3, name: "JJY", carriers: [40_000, 60_000], am: false, about: N_("Japan: carrier on 0.8 s (0), 0.5 s (1), 0.2 s (marker); JST") },
  { id: 5, name: "RBU", carriers: [200_000 / 3], am: false, margin: 350, about: N_("Taldom near Moscow: each 0.1 s a bit, the carrier phase-modulated at 100 Hz (0) or 312.5 Hz (1); Moscow time") },
  { id: 9, name: "TDF", label: "ALS162 (TDF)", carriers: [162_000], am: false, margin: 100, about: N_("Allouis, France: DCF77's code as phase modulation of the carrier (one pulse a second for a 0, two for a 1); CET/CEST") },
  { id: 10, name: "BPC", carriers: [68_500], am: false, about: N_("Shangqiu, China: carrier reduced 0.1-0.4 s each second (two bits), none at seconds 0, 20 and 40; Beijing time") },
  { id: 6, name: "DCF39", carriers: [139_000], am: false, margin: 250, about: N_("Burg, Germany (EFR teleswitching): FSK telegrams, a time telegram every 10 s; CET/CEST") },
  { id: 7, name: "HGA22", carriers: [135_600], am: false, margin: 250, about: N_("Lakihegy, Hungary (EFR teleswitching): FSK telegrams, a time telegram every 10 s; CET/CEST") },
  { id: 8, name: "DCF49", carriers: [129_100], am: false, margin: 250, about: N_("Burg, Germany (EFR teleswitching): FSK telegrams, a time telegram every 10 s; CET/CEST") },
  { id: 4, name: "WWV/WWVH", carriers: [2_500_000, 5_000_000, 10_000_000, 15_000_000, 20_000_000, 25_000_000], am: true, about: N_("Fort Collins and Hawaii: the 100 Hz subcarrier 0.17 s (0), 0.47 s (1), 0.77 s (marker), heard in AM; UTC. Which one sent a minute is told by its 1000 or 1200 Hz tones") },
  { id: 11, name: "BPM", carriers: [2_500_000, 5_000_000, 10_000_000, 15_000_000], am: true, tone: 1000, unnamed: true, about: N_("Pucheng, China: no time code, a 1 kHz tick each second (sent 20 ms early), heard in AM; minutes named by this clock") },
  { id: 12, name: "RWM", carriers: [4_996_000, 9_996_000, 14_996_000], am: false, unnamed: true, about: N_("Taldom near Moscow: no time code, the carrier on 0.1 s each second and 0.5 s at the minute in some minutes of the hour; minutes named by this clock") },
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
        if (am && Math.abs(dial - c) <= 3000) return { station: s, tone: s.tone ?? WWV_TONE, carrier: c };
        continue;
      }
      const a = audioOf(mode, dial, pitch, c);
      // in the passband (as audio: LSB's mirrored, CW's at its pitch), clear of its edges
      const [blo, bhi] = mode === "lsb" ? [-hi, -lo] : mode === "cw" ? [lo + pitch, hi + pitch] : [lo, hi];
      const room = s.margin ?? 50;
      if (a !== null && a >= Math.max(blo, 0) + room && a <= bhi - room && a >= 100) return { station: s, tone: a, carrier: c };
    }
  }
  if (want !== "auto") {
    const s = list[0];
    if (!s) return { error: t("no station {name}", { name: want }) };
    // i18n: AM, USB, LSB, CW are mode names (kept in English); a station's carrier is its unmodulated radio frequency
    return {
      error: s.am
        ? t("{name}: tune AM to {mhz} MHz", { name: tcLabel(s), mhz: s.carriers.map((c) => c / 1e6).join(", ") })
        : t("{name}: tune its carrier ({khz} kHz) into the passband in USB, LSB or CW", { name: tcLabel(s), khz: s.carriers.map((c) => c / 1e3).join(" or ") }),
    };
  }
  return { error: t("No time station in the passband: pick one to tune to.") };
}

/** How to tune a station: USB with its carrier at 1 kHz in a narrow
 * passband, or AM on WWV. */
export function tcTuning(s: TcStation, carrier = s.carriers[0]): { dial: number; mode: Mode; lo: number; hi: number } {
  if (s.am) return { dial: carrier, mode: "am", lo: -5000, hi: 5000 };
  // the sidebands as well (RBU's at ±312.5 Hz)
  const half = Math.max(400, (s.margin ?? 0) + 150);
  return { dial: carrier - 1000, mode: "usb", lo: 1000 - half, hi: 1000 + half };
}

/** What the worker posts. */
export interface TcRun {
  sec: { t: number; s: string; q: number; i: number }[];
  min: { t: number; utc: number; ok: boolean; confirmed: boolean; named?: boolean; tx?: string | null; local: string; notes: string[]; sym: string }[];
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
  /** `late` from the server's timestamps (antenna time), not estimated
   * from when the audio arrived. */
  timed?: boolean;
}

/** A minute's start (Unix s): its own time, or for a station without a
 * time code the clock's minute nearest when it was heard (`heard`), if
 * within 20 s; null when it can't be named. */
export function minuteUtc(named: boolean, utc: number, heard: number | null): number | null {
  if (named) return utc;
  if (heard === null) return null;
  const r = Math.round(heard / 60) * 60;
  return Math.abs(heard - r) <= 20 ? r : null;
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
