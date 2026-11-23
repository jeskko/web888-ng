// SPDX-License-Identifier: GPL-3.0-or-later
// Signal identification in the browser (pure parts, unit-tested): the
// classes as people call them, the decoder each one opens, and the rows
// listed from the identifier's runs (sigid/worker.ts runs
// crates/sigid-wasm: the signals in the passband named by a small CNN).

import { N_ } from "./i18n.ts";

/** A class of crates/sigid (its CLASSES). */
export type SigClass =
  | "noise"
  | "carrier"
  | "cw"
  | "fsk"
  | "psk31"
  | "ft8"
  | "ft4"
  | "wspr"
  | "ale"
  | "stanag4285"
  | "mil110a"
  | "sstv"
  | "fax"
  | "ssb"
  | "am"
  | "interference"
  | "radar"
  | "hfdl"
  | "olivia"
  | "mfsk"
  | "dominoex"
  | "hell"
  | "pactor"
  | "multitone"
  | "ale3g";

/** The decoder in this page a class opens (none: listed only). */
export type Opens = "cw" | "rtty" | "psk" | "img" | "ft8" | "ft4" | "wspr" | "hfdl" | "ssb" | "am" | null;

// i18n: signal classes the identifier names (label) and what each is (hint); mode names stay English, bd/Bd = baud (symbol rate)
export const CLASS_INFO: Record<SigClass, { label: string; opens: Opens; hint: string }> = {
  noise: { label: N_("Noise"), opens: null, hint: N_("nothing recognisable at this frequency") },
  carrier: { label: "Carrier", opens: null, hint: N_("an unmodulated carrier") },
  cw: { label: "CW", opens: "cw", hint: N_("Morse") },
  fsk: { label: "RTTY / FSK", opens: "rtty", hint: N_("two-tone FSK: RTTY, SITOR-B/NAVTEX, ASCII (the RTTY panel works out baud rate, shift and code)") },
  psk31: { label: "PSK31", opens: "psk", hint: N_("BPSK 31-125 Bd: PSK31, PSK63, PSK125") },
  ft8: { label: "FT8", opens: "ft8", hint: N_("FT8") },
  ft4: { label: "FT4", opens: "ft4", hint: N_("FT4") },
  wspr: { label: "WSPR", opens: "wspr", hint: N_("WSPR beacons") },
  ale: { label: "ALE", opens: null, hint: N_("2G ALE (MIL-STD-188-141), 8-FSK 125 Bd (not decoded here)") },
  stanag4285: { label: "STANAG 4285", opens: null, hint: N_("2400 Bd PSK modem, 106.7 ms frames (not decoded here)") },
  mil110a: { label: "MIL-110A", opens: null, hint: N_("MIL-STD-188-110A serial-tone modem, 2400 Bd PSK (not decoded here)") },
  sstv: { label: "SSTV", opens: "img", hint: N_("slow-scan television") },
  fax: { label: N_("HF fax"), opens: "img", hint: N_("weather fax (radiofax)") },
  ssb: { label: N_("SSB voice"), opens: "ssb", hint: N_("single-sideband speech") },
  am: { label: "AM", opens: "am", hint: N_("AM broadcast or voice") },
  interference: { label: N_("Interference"), opens: null, hint: N_("local noise: carriers with noisy skirts, carrier pairs and combs, wandering noise bands") },
  radar: { label: N_("Radar"), opens: null, hint: N_("over-the-horizon radar or a sounder: sweeps or pulses repeating 5-60 times a second") },
  hfdl: { label: "HFDL", opens: "hfdl", hint: N_("aircraft data link, 1800 Bd PSK bursts") },
  olivia: { label: "Olivia", opens: null, hint: N_("Olivia or Contestia: 4-64 tones over 125-2000 Hz (not decoded here)") },
  mfsk: { label: "MFSK", opens: null, hint: N_("MFSK8/16/32/64: 16 or 32 tones, the spacing the symbol rate (not decoded here)") },
  dominoex: { label: "DominoEX", opens: null, hint: N_("DominoEX or THOR: 18 tones, incremental frequency keying (not decoded here)") },
  hell: { label: "Hell", opens: null, hint: N_("Feld Hell: letters drawn by on-off keying, read by eye (not decoded here)") },
  pactor: { label: "PACTOR", opens: null, hint: N_("PACTOR I-III: data in ARQ bursts on a 1.25 s cycle (not decoded here)") },
  // i18n: parallel-tone: many carriers at once
  multitone: { label: N_("Multi-tone modem"), opens: null, hint: N_("many carriers at once: MT63, the 39-tone modem, OFDM data such as VARA (not decoded here)") },
  ale3g: { label: "3G ALE", opens: null, hint: N_("3G ALE (MIL-STD-188-141B): 2400 Bd PSK bursts (not decoded here)") },
};

/** One identification run (crates/sigid-wasm's JSON). */
export interface SigRun {
  sig: { id: number; hz: number; lo: number; hi: number; db: number; runs: number; top: [string, number][] }[];
}

export interface SigRow {
  id: number;
  /** RF of the signal's centre and its edges (Hz). */
  rf: number;
  rfLo: number;
  rfHi: number;
  /** Audio frequency of its centre (Hz). */
  hz: number;
  db: number;
  /** Likeliest classes, best first (none yet: not identified). */
  top: { cls: SigClass; p: number }[];
}

/** The rows of a run; `rf` maps audio Hz to RF (the mode's sideband).
 * Unknown class names (a newer model) are dropped. */
export function toSigRows(run: SigRun, rf: (hz: number) => number): SigRow[] {
  return run.sig
    .map((s) => {
      const [a, b] = [rf(s.lo), rf(s.hi)];
      return {
        id: s.id,
        rf: rf(s.hz),
        rfLo: Math.min(a, b),
        rfHi: Math.max(a, b),
        hz: s.hz,
        db: s.db,
        top: s.top.filter(([c]) => c in CLASS_INFO).map(([c, p]) => ({ cls: c as SigClass, p })),
      };
    })
    .sort((x, y) => x.rf - y.rf);
}

/** How sure to call it: below this the row says "unsure". */
export const SURE = 0.6;
