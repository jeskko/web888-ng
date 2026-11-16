// SPDX-License-Identifier: GPL-3.0-or-later
// Signal identification in the browser (pure parts, unit-tested): the
// classes as people call them, the decoder each one opens, and the rows
// listed from the identifier's runs (sigid/worker.ts runs
// crates/sigid-wasm: the signals in the passband named by a small CNN).

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
  | "hfdl";

/** The decoder in this page a class opens (none: listed only). */
export type Opens = "cw" | "rtty" | "psk" | "img" | "ft8" | "ft4" | "wspr" | "hfdl" | "ssb" | "am" | null;

export const CLASS_INFO: Record<SigClass, { label: string; opens: Opens; hint: string }> = {
  noise: { label: "Noise", opens: null, hint: "nothing recognisable at this frequency" },
  carrier: { label: "Carrier", opens: null, hint: "an unmodulated carrier" },
  cw: { label: "CW", opens: "cw", hint: "Morse" },
  fsk: { label: "RTTY / FSK", opens: "rtty", hint: "two-tone FSK: RTTY, SITOR-B/NAVTEX, ASCII (the RTTY panel works out baud rate, shift and code)" },
  psk31: { label: "PSK31", opens: "psk", hint: "BPSK 31-125 Bd: PSK31, PSK63, PSK125" },
  ft8: { label: "FT8", opens: "ft8", hint: "FT8" },
  ft4: { label: "FT4", opens: "ft4", hint: "FT4" },
  wspr: { label: "WSPR", opens: "wspr", hint: "WSPR beacons" },
  ale: { label: "ALE", opens: null, hint: "2G ALE (MIL-STD-188-141), 8-FSK 125 Bd (not decoded here)" },
  stanag4285: { label: "STANAG 4285", opens: null, hint: "2400 Bd PSK modem, 106.7 ms frames (not decoded here)" },
  mil110a: { label: "MIL-110A", opens: null, hint: "MIL-STD-188-110A serial-tone modem, 2400 Bd PSK (not decoded here)" },
  sstv: { label: "SSTV", opens: "img", hint: "slow-scan television" },
  fax: { label: "HF fax", opens: "img", hint: "weather fax (radiofax)" },
  ssb: { label: "SSB voice", opens: "ssb", hint: "single-sideband speech" },
  am: { label: "AM", opens: "am", hint: "AM broadcast or voice" },
  interference: { label: "Interference", opens: null, hint: "local noise: carriers with noisy skirts, carrier pairs and combs, wandering noise bands" },
  radar: { label: "Radar", opens: null, hint: "over-the-horizon radar or a sounder: sweeps or pulses repeating 5-60 times a second" },
  hfdl: { label: "HFDL", opens: "hfdl", hint: "aircraft data link, 1800 Bd PSK bursts" },
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
