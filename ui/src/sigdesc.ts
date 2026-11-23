// SPDX-License-Identifier: GPL-3.0-or-later
// "What is this?": a few sentences about one signal, from what
// crates/sigid measured of it (its extent, tones, keying, symbol rate,
// phase order, frames, drift: sigid/src/measure.rs) and what the
// identifier (a CNN) named it, as an analyst would put them together:
// the figures matched against the modes they fit, checked against the
// identifier's guess (agreeing: sure; contradicting: said so, and the
// measurements win), then where it is (the band plan, the labels near
// it). Pure, unit-tested; every text through i18n.

import { formatKhz } from "./fmt.ts";
import { t } from "./i18n.ts";
import type { BandPlan, FreqMarker } from "./markers.ts";
import { useName } from "./markers.ts";
import { CLASS_INFO, SURE, type SigClass } from "./sigid.ts";
import { MODES as DIGI } from "./digi.ts";

/** crates/sigid-wasm's analysis (audio Hz). */
export interface SigAn {
  lo: number;
  hi: number;
  centre: number;
  bw90: number;
  snr: number;
  fills: boolean;
  duty: number;
  bursts: number;
  onMean: number;
  onShort: number;
  /** On over off (dB, medians): keying drops to the noise. */
  depth?: number;
  envCv: number;
  syllabic: number;
  /** Peak to average power (dB). */
  papr: number;
  /** Power kurtosis, the noise taken out: 1 constant envelope, 2
   * Gaussian-like (OFDM). */
  kurt?: number;
  /** [audio Hz, dB over the floor], strongest first. */
  tones: [number, number][];
  spacing: number | null;
  /** Tones seen frame by frame: [spacing Hz, tones, strength, frame s,
   * changes/s]. */
  grid?: [number, number, number, number, number] | null;
  /** Two-tone FSK's shift from its frequency track (Hz). */
  shift: number | null;
  line: number;
  lineAt: number;
  sideLo: number;
  sideHi: number;
  /** Below and above the line alike (AM: ~1). */
  mirror?: number;
  /** Narrow lines with 5 % of the power or more [Hz, share]. */
  carriers?: [number, number][];
  /** [Bd, dB the line stands out] */
  baudEnv: [number, number] | null;
  baudFreq: [number, number] | null;
  /** [order, carrier offset Hz, line share] */
  psk: [number, number, number] | null;
  /** [s, correlation] */
  period: [number, number] | null;
  envPeriod: [number, number] | null;
  /** A narrow |R| peak at 2-40 ms: OFDM's useful symbol [s, r]. */
  symbol?: [number, number] | null;
  drift: number | null;
  /** A sweep: [Hz/s, bursts, r2, burst s, sweeps/s from flybacks]. */
  chirp?: [number, number, number, number, number | null] | null;
  top: [string, number][];
}

export interface DescCtx {
  /** Audio Hz to RF Hz (the mode's sideband). */
  rf: (hz: number) => number;
  plan: BandPlan | null;
  markers: FreqMarker[];
}

export type Sureness = "sure" | "likely" | "unsure";

export interface Desc {
  /** What it most likely is. */
  title: string;
  sure: Sureness;
  /** The class whose decoder (or tuning) fits, if any. */
  cls: SigClass | null;
  /** RF of its centre and edges (Hz). */
  rf: number;
  rfLo: number;
  rfHi: number;
  /** What it looks like and why, then where it is. */
  text: string[];
  /** The figures: [label, value]. */
  facts: [string, string][];
  /** It fills the passband: a wide look would see more. */
  fills: boolean;
}

/** What the figures say, before naming anything. */
export interface Features {
  weak: boolean;
  carrier: boolean;
  keyed: boolean;
  /** A carrier with sidebands on both sides / on one side only. */
  am: boolean;
  amOneSide: boolean;
  speech: boolean;
  /** Two tones: their shift (Hz); the gap between their spectral peaks. */
  shift: number | null;
  toneGap: number | null;
  /** Several tones on one grid: how many, how far apart, how often the
   * strongest moves (per s, frame by frame). */
  mfsk: { n: number; spacing: number; changes: number | null } | null;
  /** Symbol rate (Bd). */
  baud: number | null;
  pskOrder: number | null;
  /** Repetition of the signal / pulses (s). */
  frame: number | null;
  pulses: number | null;
  steady: boolean;
  /** Typical dit (s) when keyed. */
  dit: number | null;
  /** Its width (Hz) and whether it fills the passband. */
  width: number;
  fills: boolean;
  /** OFDM: a pattern repeating at the useful symbol (s), the amplitude
   * noise-like, no carrier. */
  ofdm: number | null;
  /** Many carriers at once on one grid. */
  parallel: { n: number; spacing: number } | null;
  /** Narrow (90 % of the power within 12 Hz) and steady in level. */
  narrow: boolean;
  /** Carriers side by side (Hz as measured), strongest first. */
  carriers: number[];
  /** A sweep: slope (Hz/s), repetitions a second (null: once), pulsed,
   * its width (Hz: slope x pulse, or slope / rate). */
  sweep: { slope: number; rate: number | null; pulsed: boolean; width: number | null; once: boolean } | null;
  /** Bursts: on fraction, mean burst (s). */
  duty: number;
  onMean: number;
}

const near = (x: number | null | undefined, v: number, tol: number) => x != null && Math.abs(x - v) <= tol * v;

/** A sweep's figures: pulsed (on less than 60 % of the time, keyed down
 * to the noise) its pulses each a sweep (slope x pulse wide), else FMCW
 * crossing the window once a sweep (slope / rate wide). */
function sweepOf(a: SigAn): Features["sweep"] {
  const c = a.chirp;
  if (!c) return null;
  const once = c[1] <= 1;
  // (flybacks only say the rate of a sweep that is always on: one inside
  // the window)
  const inside = c[4] != null && a.duty >= 0.9;
  const rate = once ? null : inside ? c[4]! : a.envPeriod ? 1 / a.envPeriod[0] : c[1] / 4.096;
  // a burst sweeping across all of a window it fills is a wider sweep
  // crossing it, not a pulse
  const crossing = a.fills && Math.abs(c[0]) * c[3] >= 0.85 * (a.hi - a.lo);
  const pulsed = !once && !crossing && !inside && a.duty < 0.6 && (a.depth ?? 0) >= 15;
  const width = once ? null : pulsed ? Math.abs(c[0]) * c[3] : rate ? Math.abs(c[0]) / rate : null;
  // a sweep can't be wider than the signal it was measured in (unless it
  // crosses a window it fills): a narrow carrier's wobble, not a radar
  if (!a.fills && width != null && width > 1.5 * (a.hi - a.lo)) return null;
  return { slope: c[0], rate, pulsed, width, once };
}

/** HF radars and sounders by sigidwiki.com's figures (2026-11): pulsed
 * or FMCW, sweeps a second, sweep width (Hz), direction, where. */
const RADARS: { name: string; pulsed: boolean; rates: number[]; bw: [number, number]; dir?: 1 | -1; mhz?: [number, number] }[] = [
  { name: "29B6 'Kontayner'", pulsed: true, rates: [40, 50, 25, 100], bw: [9e3, 20e3], dir: 1, mhz: [6.1, 32] },
  { name: "29B6 'Kontayner' sounder", pulsed: true, rates: [10], bw: [8e3, 16e3], dir: -1, mhz: [6.1, 32] },
  { name: "OTH-SW", pulsed: false, rates: [43, 86], bw: [35e3, 90e3], mhz: [6, 30] },
  { name: "Podsolnukh 'Sunflower'", pulsed: false, rates: [20, 30, 43, 87], bw: [35e3, 65e3], mhz: [3.5, 8] },
  { name: "ROTHR", pulsed: false, rates: [25, 32, 42], bw: [3e3, 110e3], mhz: [5, 28] },
  { name: "JORN", pulsed: false, rates: [67], bw: [2.5e3, 65e3], mhz: [5.7, 33] },
  { name: "Chinese 'Foghorn'", pulsed: false, rates: [42, 48, 50, 67, 83], bw: [8e3, 22e3], mhz: [6, 29] },
  { name: "the Chinese 160 kHz OTH radar", pulsed: false, rates: [10], bw: [120e3, 400e3], mhz: [6, 29] },
];

/** The radars a sweep's figures fit (at `rf` Hz). */
export function radarsFor(s: NonNullable<Features["sweep"]>, rf: number): string[] {
  if (s.once || s.rate == null || s.width == null) return [];
  return RADARS.filter(
    (r) =>
      r.pulsed === s.pulsed &&
      r.rates.some((v) => near(s.rate, v, 0.04)) &&
      s.width! >= r.bw[0] &&
      s.width! <= r.bw[1] &&
      (r.dir == null || Math.sign(s.slope) === r.dir) &&
      (r.mhz == null || (rf >= r.mhz[0] * 1e6 && rf <= r.mhz[1] * 1e6)),
  ).map((r) => r.name);
}

export function features(a: SigAn): Features {
  const width = a.hi - a.lo;
  const [t0, t1] = a.tones;
  // two tones: the strongest two within 12 dB (fax's black and white
  // need not be equal), every other one 6 dB under the weaker or between
  // them (FSK's transitions)
  const twoShape =
    !!t0 &&
    !!t1 &&
    Math.abs(t0[1] - t1[1]) < 12 &&
    Math.abs(t0[0] - t1[0]) <= 1000 &&
    a.tones.slice(2).every(([h, d]) => d < Math.min(t0[1], t1[1]) - 6 || (h - t0[0]) * (h - t1[0]) < 0) &&
    a.line < 0.5;
  // speech has no symbols; fading FSK's level moves as speech's, a
  // voice's spectrum can have two peaks: FSK when its tones switch at a
  // rate
  const fskRate = twoShape && a.baudFreq != null;
  // (weak speech: its level blocks seldom all on, its kurtosis high)
  const speech = a.line < 0.25 && a.syllabic >= 0.35 && (a.envCv >= 0.15 || (a.kurt ?? 0) >= 1.5) && width >= 600 && !fskRate;
  // (a clear grid of four or more tones is MFSK, whatever two spectral
  // peaks stand out)
  const g = a.grid;
  const gridMany = !!g && g[1] >= 4 && g[2] >= 0.5;
  const two = twoShape && !speech && (a.envCv < 0.25 || fskRate) && !gridMany;
  // hopping tones (one at a time: a constant envelope) on a grid, seen
  // frame by frame; else peaks of the whole spectrum on a grid
  // the amplitude's statistics, noise taken out (else peak to average)
  const papr = a.papr ?? 0;
  const constant = a.kurt != null && a.kurt > 0 ? a.kurt < 1.15 : papr < 3.5;
  const gaussian = a.kurt != null && a.kurt > 0 ? a.kurt >= 1.7 : papr >= 5.5;
  const swept = !!a.chirp;
  const mfsk = swept
    ? null
    : g && g[1] >= 4 && constant && !a.psk && !speech && !two && a.line < 0.5
      ? { n: g[1], spacing: g[0], changes: g[4] }
      : a.spacing != null && a.tones.length >= 3 && a.envCv < 0.25 && !two && !speech
        ? { n: a.tones.length, spacing: a.spacing, changes: null }
        : null;
  // keyed: on and off in whole elements that make up most of its time on
  // (not a steady signal with a few drop-outs)
  // (and down to the noise: a fading carrier's dips are shallower)
  const keyed =
    a.duty > 0.15 &&
    a.duty < 0.9 &&
    a.bursts >= 4 &&
    a.line >= 0.3 &&
    a.onShort >= 0.003 &&
    a.bursts * a.onMean >= 0.4 * a.duty * 4.096 &&
    (a.depth ?? 99) >= 15;
  const sides = [a.sideLo, a.sideHi].sort((x, y) => x - y);
  // AM: sidebands on both sides, each other's mirror image
  const am = a.line >= 0.3 && sides[0] >= 0.005 && sides[0] * 4 >= sides[1] && (a.mirror ?? 1) >= 0.5 && !keyed;
  const amOneSide = a.line >= 0.3 && sides[1] >= 0.005 && sides[0] * 4 < sides[1] && !keyed;
  const carrier = a.line >= 0.8 && a.sideLo + a.sideHi < 0.005 && a.duty >= 0.9;
  // FSK's rate is in its frequency changes (its envelope's lines are the
  // filter's), PSK's in its envelope
  const baud = two || mfsk ? (a.baudFreq?.[0] ?? a.baudEnv?.[0] ?? null) : (a.baudEnv?.[0] ?? null);
  return {
    weak: a.snr < 3 && !a.fills,
    carrier,
    keyed,
    am,
    amOneSide,
    speech,
    shift: two ? (a.shift ?? Math.abs(t0[0] - t1[0])) : null,
    toneGap: two ? Math.abs(t0[0] - t1[0]) : null,
    mfsk,
    baud: carrier || am || speech ? null : baud,
    pskOrder: a.psk?.[0] ?? null,
    frame: speech || swept ? null : (a.period?.[0] ?? null),
    pulses: a.envPeriod && a.duty < 0.85 && !keyed ? a.envPeriod[0] : null,
    steady: a.envCv < 0.1 && a.duty >= 0.9 && !keyed && a.line < 0.3 && !swept,
    dit: keyed ? a.onShort : null,
    width,
    fills: a.fills,
    ofdm: !a.chirp && (a.symbol ?? a.period) && a.line < 0.05 && gaussian && width >= 1500 && !speech && !two ? (a.symbol ?? a.period)![0] : null,
    parallel: swept
      ? null
      : g && g[1] >= 8 && gaussian && !speech && a.line < 0.3
        ? { n: g[1], spacing: g[0] }
        : a.spacing != null && a.tones.length >= 8 && (a.envCv >= 0.25 || papr >= 4) && !speech && a.line < 0.3
          ? { n: a.tones.length, spacing: a.spacing }
          : null,
    carriers: (a.carriers ?? [])
      .filter(([, sh]) => sh >= 0.1)
      .map(([h]) => h)
      .filter((h, i, all) => all.slice(0, i).every((o) => Math.abs(o - h) >= 1000)),
    narrow: a.bw90 <= 12 && !keyed && constant,
    sweep: sweepOf(a),
    duty: a.duty,
    onMean: a.onMean,
  };
}

/** A mode the figures fit: its name, class, and how specific the match
 * is (exact: standard figures; family: a kind of signal). */
export interface Match {
  title: string;
  cls: SigClass | null;
  exact: boolean;
  /** The mode's symbol rate (Bd; null: it has none, as fax): a measured
   * rate that doesn't fit it is left unsaid. */
  rate?: number | null;
}

const RTTY_BAUD = [45.45, 50, 75];
const RTTY_SHIFT = [85, 170, 200, 425, 450, 850];

/** The modes the figures fit, most specific first (`rf`: where, Hz). */
export function matches(f: Features, rf = 0): Match[] {
  const out: Match[] = [];
  // radars and sounders, by their sweeps
  if (f.sweep) {
    const sw = f.sweep;
    const fit = radarsFor(sw, rf);
    if (sw.once && Math.abs(sw.slope) >= 20e3 && Math.abs(sw.slope) <= 1e6)
      // i18n: a chirp ionosonde: sweeps the whole HF range once every few minutes to sound the ionosphere
      out.push({ title: t("A single sweep at {rate} kHz/s: a chirp ionosonde", { rate: Math.round(Math.abs(sw.slope) / 1000) }), cls: "radar", exact: true, rate: null });
    else if (fit.length)
      out.push({ title: t("OTH radar: fits {names}", { names: fit.join(t(" or ")) }), cls: "radar", exact: true, rate: null });
    else if (sw.rate != null)
      out.push({ title: t("A sweeping radar or sounder ({rate} sweeps a second)", { rate: sw.rate.toFixed(1) }), cls: "radar", exact: false, rate: null });
  }
  const b = f.baud;
  const n = (v: number) => (Math.abs(v - Math.round(v)) < 0.05 ? String(Math.round(v)) : v.toFixed(2));
  if (f.shift != null && b != null) {
    const sh = RTTY_SHIFT.find((s) => near(f.shift, s, 0.12));
    const bd = RTTY_BAUD.find((r) => near(b, r, 0.03));
    if (near(b, 100, 0.03) && near(f.shift, 170, 0.12) && f.duty >= 0.9)
      // i18n: SITOR-B: the maritime FEC teleprinter mode NAVTEX uses; Bd = baud
      out.push({ title: t("SITOR-B / NAVTEX (100 Bd, 170 Hz shift)"), cls: "fsk", exact: true, rate: 100 });
    else if ((near(b, 100, 0.03) || near(b, 200, 0.03)) && near(f.shift, 200, 0.12) && f.duty < 0.9 && f.duty > 0.5)
      out.push({ title: t("PACTOR-I ({baud} Bd, 200 Hz shift, in ARQ bursts)", { baud: Math.round(b) }), cls: "pactor", exact: true, rate: Math.round(b) });
    else if (bd != null && near(b, 50, 0.03) && near(f.shift, 450, 0.1))
      // i18n: the German weather service's teleprinter broadcasts
      out.push({ title: t("RTTY 50 Bd, 450 Hz shift (as the DWD weather broadcasts)"), cls: "fsk", exact: true, rate: 50 });
    else if (bd != null && sh != null) out.push({ title: t("RTTY {baud} Bd, {shift} Hz shift", { baud: n(bd), shift: sh }), cls: "fsk", exact: true, rate: bd });
    else if (sh != null && [110, 150, 300].some((r) => near(b, r, 0.03)))
      out.push({ title: t("ASCII teleprinter, {baud} Bd, {shift} Hz shift", { baud: n(b), shift: Math.round(f.shift) }), cls: "fsk", exact: true });
    else {
      // utility FSK by its rate and shift (sigidwiki.com): named as what it
      // fits, since others may use the same figures
      const fsk = (baud: number, shift: number) => near(b, baud, 0.03) && near(f.shift, shift, 0.1);
      const cis36 =
        (near(b, 36, 0.03) && [85, 125, 250, 500].some((v) => near(f.shift, v, 0.1))) ||
        ([50, 75, 100, 150].some((v) => near(b, v, 0.03)) && [125, 250, 500].some((v) => near(f.shift, v, 0.1)));
      const said = { baud: n(b), shift: Math.round(f.shift) };
      if (cis36)
        // i18n: CIS-36-50 (also BEE-36): a Russian Navy FSK teleprinter mode
        out.push({ title: t("FSK {baud} Bd, {shift} Hz shift: fits CIS-36-50 (Russian Navy)", said), cls: "fsk", exact: true, rate: b });
      else if (fsk(200, 500) || fsk(200, 1000))
        // i18n: F01, F06: catalogue names of Russian intelligence FSK broadcasts
        out.push({ title: t("FSK {baud} Bd, {shift} Hz shift: fits the Russian F01 / F06 broadcasts", said), cls: "fsk", exact: true, rate: 200 });
      else if (fsk(100, 625))
        // i18n: F11: catalogue name of a Polish intelligence FSK broadcast
        out.push({ title: t("FSK {baud} Bd, {shift} Hz shift: fits the Polish F11 broadcasts", said), cls: "fsk", exact: true, rate: 100 });
    }
  }
  if (f.toneGap != null && near(f.toneGap, 800, 0.1))
    // i18n: HF fax: weather charts sent by radio; black and white are two frequencies 800 Hz apart
    out.push({ title: t("HF fax (black and white 800 Hz apart)"), cls: "fax", exact: true, rate: null });
  if (f.mfsk && near(f.mfsk.spacing, 250, 0.05) && (b == null || near(b, 125, 0.05) || near(b, 250, 0.05)))
    // i18n: 2G ALE: automatic link establishment, MIL-STD-188-141; 8-FSK = eight tones
    out.push({ title: t("2G ALE (8-FSK, 250 Hz apart, 125 Bd)"), cls: "ale", exact: true, rate: 125 });
  if (b != null && near(b, 1800, 0.01)) out.push({ title: t("HFDL (1800 Bd PSK, aircraft data link)"), cls: "hfdl", exact: true, rate: 1800 });
  if (b != null && near(b, 2400, 0.01) && f.duty < 0.7 && !near(f.frame, 0.10667, 0.03))
    out.push({ title: t("2400 Bd PSK in bursts (as 3G ALE)"), cls: "ale3g", exact: true, rate: 2400 });
  else if (near(f.frame, 0.10667, 0.03))
    out.push({ title: t("STANAG 4285 (2400 Bd PSK, 106.7 ms frames)"), cls: "stanag4285", exact: true, rate: 2400 });
  else if (b != null && near(b, 2400, 0.01))
    // i18n: serial-tone: one carrier, fast PSK symbols (MIL-STD-188-110A, STANAG 4285...)
    out.push({ title: t("2400 Bd serial-tone PSK modem (as MIL-STD-188-110A)"), cls: "mil110a", exact: true, rate: 2400 });
  for (const [r, name] of [
    [31.25, "PSK31"],
    [62.5, "PSK63"],
    [125, "PSK125"],
  ] as const) {
    if (near(b, r, 0.02) && (f.pskOrder === 2 || f.pskOrder == null) && !f.shift && !f.mfsk)
      out.push({ title: f.pskOrder === 2 ? t("{mode} (BPSK, {baud} Bd)", { mode: name, baud: r }) : name, cls: "psk31", exact: f.pskOrder === 2, rate: r });
  }
  // DRM: OFDM whose useful symbol is one of its four robustness modes'
  if (f.ofdm != null) {
    const drm = (
      [
        ["A", 0.024],
        ["B", 0.021333],
        ["C", 0.014667],
        ["D", 0.009333],
      ] as const
    ).find(([, tu]) => near(f.ofdm, tu, 0.02));
    if (drm)
      // i18n: DRM = Digital Radio Mondiale, digital shortwave broadcasting; OFDM = many carriers; mode A-D = its robustness modes
      out.push({ title: t("DRM digital broadcast (OFDM, mode {m}, {ms} ms symbols)", { m: drm[0], ms: (1000 * drm[1]).toFixed(2) }), cls: null, exact: true, rate: null });
    else out.push({ title: t("OFDM, {ms} ms symbols", { ms: (1000 * f.ofdm).toFixed(1) }), cls: "multitone", exact: false, rate: null });
  }
  if (b != null && near(b, 44.44, 0.02) && f.width >= 1900 && f.width <= 2700 && !f.parallel)
    out.push({ title: t("39-tone HF modem (44.44 Bd)"), cls: "multitone", exact: true, rate: 44.44 });
  if (f.parallel) {
    const { n, spacing } = f.parallel;
    if (near(spacing, 56.25, 0.03)) out.push({ title: t("39-tone HF modem (56.25 Hz apart)"), cls: "multitone", exact: true, rate: 44.44 });
    else if (near(spacing, 112.5, 0.03) && n >= 10 && n <= 17)
      // i18n: Codan: an HF radio maker; its data modem has 16 DPSK channels
      out.push({ title: t("Codan data modem (16 channels 112.5 Hz apart)"), cls: "multitone", exact: true, rate: null });
    else if (near(spacing, 80, 0.03) && n >= 20 && n <= 33)
      // i18n: CHIRP: Codan's ALE (link establishment), 32 channels
      out.push({ title: t("Codan CHIRP ALE (32 channels 80 Hz apart)"), cls: "multitone", exact: true, rate: null });
    else if ([7.8125, 15.625, 31.25].some((v) => near(spacing, v, 0.03)) && n >= 20)
      out.push({ title: t("MT63 ({width} wide)", { width: `${Math.round((64 * spacing) / 100) * 100} Hz` }), cls: "multitone", exact: true, rate: null });
    else out.push({ title: t("{n} carriers {spacing} Hz apart (a parallel-tone modem)", { n, spacing: spacing.toFixed(1) }), cls: "multitone", exact: false });
  }
  // CLOVER 2000: 2 kHz, 62.5 Bd, eight tone pulses 2 ms apart (a 16 ms cycle)
  const per = f.ofdm ?? f.frame;
  if (f.width >= 1600 && f.width <= 2500 && (near(b, 62.5, 0.03) || near(per, 0.016, 0.05)) && !f.mfsk)
    // i18n: CLOVER 2000: an HF data mode by HAL Communications
    out.push({ title: t("CLOVER 2000 (62.5 Bd, a 16 ms cycle)"), cls: "multitone", exact: false, rate: 62.5 });
  // Chinese-64 MFSK: ~64 tones in 2.5 kHz, 18.75 or 37.5 Bd
  if (f.mfsk && f.mfsk.n >= 40 && f.width >= 1900 && f.width <= 2900) {
    const r = b ?? f.mfsk.changes;
    if (r == null || near(r, 18.75, 0.1) || near(r, 37.5, 0.1))
      // i18n: a Chinese military MFSK mode (as sigidwiki.com names it)
      out.push({ title: t("{n} tones {spacing} Hz apart: fits Chinese-64 MFSK", { n: f.mfsk.n, spacing: f.mfsk.spacing.toFixed(1) }), cls: "mfsk", exact: true, rate: null });
  }
  // the MFSK families, by their tone grid (spacing, tones used) and the
  // rate on it (measured, or how often the tone moves frame by frame)
  if (f.mfsk && !f.parallel) {
    const { n, spacing: sp, changes } = f.mfsk;
    const r = b ?? changes;
    const rateOk = (x: number) => r == null || near(r, x, 0.25) || near(r, 2 * x, 0.1);
    const w = f.width;
    const mf = (
      [
        [7.8125, "MFSK8", 32],
        [15.625, "MFSK16", 16],
        [31.25, "MFSK32", 16],
        [62.5, "MFSK64", 16],
      ] as const
    ).find(([v, , m]) => near(sp, v, 0.03) && n >= m - 3 && n <= m && rateOk(v));
    const dom = [7.8125, 10.766, 15.625, 21.533].some((v) => near(sp, v, 0.03)) && n >= 15 && n <= 18;
    // Olivia's and Contestia's tones/bandwidth pairs
    const OL = [
      [4, 125],
      [4, 250],
      [8, 250],
      [8, 500],
      [16, 500],
      [8, 1000],
      [16, 1000],
      [32, 1000],
      [64, 2000],
    ] as const;
    const olm = OL.find(([m, bw]) => Math.abs(n - m) <= Math.max(1, m / 8) && near(sp, bw / m, 0.1));
    const [ol, olBw] = olm ?? [0, 0];
    if (near(sp, 250, 0.05) && n >= 6 && n <= 8 && (b == null || near(b, 125, 0.05) || near(b, 250, 0.05))) {
      // (2G ALE: matched below from its tones)
    } else if (near(sp, 6.25, 0.04) && n >= 6 && n <= 8)
      out.push({ title: t("8-FSK, 6.25 Hz apart (as FT8)"), cls: "ft8", exact: false, rate: 6.25 });
    else if (near(sp, 15.625, 0.03) && n >= 14 && n <= 16)
      // (DominoEX's 18 tones at this spacing, one or two unseen, look the same)
      out.push({ title: t("MFSK16 (or DominoEX / THOR)"), cls: "mfsk", exact: true, rate: null });
    else if (mf && n <= 16) out.push({ title: mf[1], cls: "mfsk", exact: true, rate: mf[0] });
    else if (dom && n >= 17)
      // i18n: DominoEX and THOR: amateur modes of 18 tones, each the last plus 2 + the data (incremental frequency keying)
      out.push({ title: t("DominoEX or THOR (18 tones {sp} Hz apart)", { sp: sp.toFixed(2) }), cls: "dominoex", exact: true, rate: null });
    else if (ol && olBw && w <= 1.4 * olBw + 100)
      // i18n: Olivia and Contestia: amateur MFSK modes, M tones over a bandwidth
      out.push({ title: t("Olivia or Contestia {m}/{bw}", { m: ol, bw: olBw }), cls: "olivia", exact: true, rate: null });
    else if (mf) out.push({ title: mf[1], cls: "mfsk", exact: true, rate: mf[0] });
  }
  if (f.keyed && f.dit && f.dit >= 0.0035 && f.dit <= 0.014)
    // i18n: Feld Hell: text drawn as pixels by on-off keying
    out.push({ title: t("Feld Hell ({ms} ms pixels)", { ms: (1000 * f.dit).toFixed(1) }), cls: "hell", exact: true, rate: null });
  if (near(f.pulses, 1.25, 0.05) || (f.duty < 0.9 && near(f.onMean, 0.96, 0.08) && f.duty > 0.5))
    // i18n: ARQ: automatic repeat request, data and short acknowledgements in turn
    out.push({ title: t("PACTOR (ARQ: 0.96 s bursts every 1.25 s)"), cls: "pactor", exact: true, rate: null });
  if (f.keyed && f.dit && f.dit > 0.014) {
    const wpm = Math.round(1.2 / f.dit);
    // i18n: CW = Morse; WPM = words per minute
    out.push({ title: t("CW (Morse), about {wpm} WPM", { wpm }), cls: "cw", exact: wpm >= 5 && wpm <= 60, rate: null });
  }
  if (f.carrier) out.push({ title: t("An unmodulated carrier"), cls: "carrier", exact: true });
  else if (f.narrow && !f.am && !f.amOneSide)
    // i18n: WSPR: a slow amateur beacon mode, 6 Hz wide
    out.push({ title: t("A narrow steady signal: a carrier, or a slow mode such as WSPR"), cls: null, exact: false, rate: null });
  if (f.carriers.length >= 2 && !f.keyed)
    out.push({ title: t("AM stations side by side ({n} carriers)", { n: f.carriers.length }), cls: "am", exact: true, rate: null });
  else if (f.am && f.width >= 6000 && !f.fills)
    out.push({ title: t("AM broadcast ({width} kHz wide)", { width: (f.width / 1000).toFixed(1) }), cls: "am", exact: true, rate: null });
  else if (f.am) out.push({ title: t("AM (a carrier with both sidebands)"), cls: "am", exact: true });
  if (f.amOneSide) out.push({ title: t("A carrier with sidebands on one side (AM heard in SSB?)"), cls: "am", exact: false });
  if (f.speech) out.push({ title: t("Speech (SSB voice)"), cls: "ssb", exact: false });
  if (f.shift != null && !out.some((m) => m.exact))
    out.push({ title: t("Two-tone FSK, {shift} Hz shift", { shift: Math.round(f.shift) }), cls: "fsk", exact: false });
  if (f.mfsk && !out.some((m) => m.exact))
    out.push({ title: t("{n} tones {spacing} Hz apart (MFSK)", { n: f.mfsk.n, spacing: f.mfsk.spacing.toFixed(1) }), cls: null, exact: false });
  if (f.pskOrder && !out.some((m) => m.exact))
    out.push({ title: t("{psk} phase modulation", { psk: pskName(f.pskOrder) }), cls: null, exact: false });
  if (f.pulses != null && !out.some((m) => m.exact))
    out.push({ title: t("Pulses, {rate} a second", { rate: (1 / f.pulses).toFixed(1) }), cls: "radar", exact: false });
  return out;
}

function pskName(k: number): string {
  return k === 2 ? "BPSK" : k === 4 ? "QPSK" : k === 8 ? "8-PSK" : `${k}-PSK`;
}

/** What a class expects of the figures (and of where it is: FT8, FT4
 * and WSPR live just above their dials): false when they contradict it. */
function fits(c: SigClass, f: Features, a: SigAn, rf: number): boolean {
  switch (c) {
    case "ft8":
    case "ft4":
    case "wspr":
      return DIGI[c].dials.some((d) => rf >= d.hz - 200 && rf <= d.hz + 3200);
    case "am":
      // a carrier, and something beside it
      return a.line >= 0.3 && a.sideLo + a.sideHi >= 0.005;
    case "carrier":
      return !f.keyed && a.line >= 0.5;
    case "ssb":
      return !f.am && !f.carrier && a.line < 0.5;
    case "cw":
      return a.line >= 0.2;
    case "fsk":
      // (two tones: a grid of more is MFSK)
      return f.shift != null || (f.mfsk == null && a.envCv < 0.25);
    case "fax":
      // black and white, about 800 Hz apart (a lone tone is not fax)
      return f.shift != null && f.shift >= 500 && f.shift <= 1000;
    case "noise":
      return a.snr < 6;
    default:
      return true;
  }
}

/** The headline, how sure, the sentences and the figures. */
export function describe(a: SigAn, ctx: DescCtx): Desc {
  const f = features(a);
  const ms = matches(f, ctx.rf(a.centre));
  const top = a.top.filter(([c]) => c in CLASS_INFO).map(([c, p]) => ({ cls: c as SigClass, p }));
  const best = top[0];
  const label = (c: SigClass) => t(CLASS_INFO[c].label);
  const text: string[] = [];
  let title: string;
  let sure: Sureness;
  let cls: SigClass | null;
  let chosen: Match | null = null;
  const exact = ms.find((m) => m.exact);
  const agree = ms.find((m) => m.cls != null && top.slice(0, 2).some((x) => x.cls === m.cls && x.p >= 0.2));
  const fit = best ? fits(best.cls, f, a, ctx.rf(a.centre)) : false;
  if (agree) {
    chosen = agree;
    title = agree.title;
    cls = agree.cls;
    sure = agree.exact || (best.cls === agree.cls && best.p >= SURE) ? "sure" : "likely";
  } else if (exact) {
    chosen = exact;
    title = exact.title;
    cls = exact.cls;
    sure = "likely";
    if (best && best.cls !== "noise")
      text.push(t("The identifier guessed {mode} ({p} %), but the figures fit this better.", { mode: label(best.cls), p: Math.round(100 * best.p) }));
  } else if (best && best.p >= SURE && fit) {
    title = label(best.cls);
    cls = best.cls;
    sure = ms.length ? "likely" : best.p >= 0.9 ? "likely" : "unsure";
  } else if (ms.length) {
    chosen = ms[0];
    title = ms[0].title;
    cls = ms[0].cls;
    sure = "unsure";
    if (best && !fit) text.push(t("The identifier guessed {mode}, which the figures don't bear out.", { mode: label(best.cls) }));
  } else if (best && fit) {
    title = top[1] && top[1].p >= 0.15 ? t("{a} or {b}?", { a: label(best.cls), b: label(top[1].cls) }) : t("{mode}?", { mode: label(best.cls) });
    cls = best.cls;
    sure = "unsure";
  } else if (best) {
    // its guess contradicted: the next one that fits, else what the
    // figures show
    const alt = top.slice(1, 3).find((x) => x.p >= 0.1 && fits(x.cls, f, a, ctx.rf(a.centre)));
    text.push(t("The identifier guessed {mode}, which the figures don't bear out.", { mode: label(best.cls) }));
    sure = "unsure";
    if (alt) {
      title = t("{mode}?", { mode: label(alt.cls) });
      cls = alt.cls;
    } else if (a.line >= 0.5) {
      title = t("A carrier with little modulation");
      cls = null;
    } else {
      title = t("Something unknown");
      cls = null;
    }
  } else {
    title = t("Something unknown");
    cls = null;
    sure = "unsure";
  }
  if (cls === "noise") cls = null;

  const rfA = ctx.rf(a.lo);
  const rfB = ctx.rf(a.hi);
  const rfLo = Math.min(rfA, rfB);
  const rfHi = Math.max(rfA, rfB);
  const rf = ctx.rf(a.centre);
  const width = a.hi - a.lo;
  const hz = (v: number) => (v >= 1000 ? t("{n} kHz", { n: (v / 1000).toFixed(v >= 10000 ? 0 : 1) }) : t("{n} Hz", { n: Math.round(v) }));

  // its shape and level
  if (a.fills) text.push(t("It fills the passband: widen the passband to see its edges (its width and SNR here are only a lower bound)."));
  else if (f.carrier || (f.keyed && a.bw90 < 60)) text.push(t("A narrow signal at {khz} kHz, {snr} dB over the noise.", { khz: formatKhz(Math.round(rf)), snr: Math.round(a.snr) }));
  else text.push(t("About {width} wide around {khz} kHz, {snr} dB over the noise.", { width: hz(width), khz: formatKhz(Math.round(rf)), snr: Math.round(a.snr) }));
  if (f.weak) text.push(t("It is weak: the figures below are rough."));
  // its time behaviour
  if (f.keyed) text.push(t("Keyed on and off: on {duty} % of the time, the shortest elements about {ms} ms.", { duty: Math.round(100 * a.duty), ms: Math.round(1000 * a.onShort) }));
  else if (a.duty < 0.85 && a.bursts >= 2) text.push(t("In bursts: on {duty} % of the time.", { duty: Math.round(100 * a.duty) }));
  else if (a.duty < 0.85) text.push(t("On {duty} % of the time.", { duty: Math.round(100 * a.duty) }));
  if (f.speech) text.push(t("Its level rises and falls at syllable rates, as speech does."));
  else if (f.steady && !f.carrier) text.push(t("Its level stays constant, as FM, FSK or a carrier."));
  // its spectrum
  if (f.carriers.length >= 2 && !f.keyed)
    text.push(
      t("Carriers at {list} kHz, their sidebands overlapping: stations side by side.", {
        list: f.carriers.map((h) => formatKhz(Math.round(ctx.rf(h)))).join(", "),
      }),
    );
  else if (f.am) text.push(t("A carrier with sidebands on both sides: AM."));
  else if (f.amOneSide)
    text.push(t("A carrier with sidebands on one side only: AM heard through an SSB filter (tune in AM to hear it), or a carrier beside SSB."));
  // (fax's levels are greys between black and white: its tones' gap)
  const shiftSaid = chosen?.cls === "fax" ? f.toneGap : f.shift;
  if (shiftSaid != null) text.push(t("It shifts between two tones {shift} Hz apart.", { shift: Math.round(shiftSaid) }));
  else if (f.mfsk && !f.parallel) text.push(t("{n} tones on a {spacing} Hz grid.", { n: f.mfsk.n, spacing: f.mfsk.spacing.toFixed(1) }));
  // the measured rate, unless the mode named has none or another
  const r = chosen?.rate;
  const rateFits = r === undefined || (r != null && f.baud != null && [r, 2 * r, r / 2].some((x) => near(f.baud, x, 0.03)));
  const shown = r ?? f.baud;
  if (shown != null && rateFits) text.push(t("Symbols at {baud} Bd.", { baud: shown >= 100 ? Math.round(shown) : +shown.toFixed(2) }));
  if (f.pskOrder) text.push(t("Phase modulation with {k} phases ({psk}).", { k: f.pskOrder, psk: pskName(f.pskOrder) }));
  if (f.ofdm != null)
    text.push(t("Its amplitude is noise-like, as many carriers at once make it (OFDM), and a pattern repeats every {ms} ms: its symbols.", { ms: (1000 * f.ofdm).toFixed(2) }));
  else if (f.parallel) text.push(t("{n} carriers on a {spacing} Hz grid at once.", { n: f.parallel.n, spacing: f.parallel.spacing.toFixed(2) }));
  if (f.sweep) {
    const sw = f.sweep;
    const dir = sw.slope > 0 ? t("upward") : t("downward");
    const k = (Math.abs(sw.slope) / 1000).toFixed(0);
    if (sw.once) text.push(t("One sweep {dir} at {rate} kHz/s through the window.", { dir, rate: k }));
    else if (sw.pulsed && sw.width != null)
      text.push(
        t("Pulses of {ms} ms, {n} a second, each sweeping {dir} at {rate} kHz/s: about {w} kHz wide.", {
          ms: (1000 * (a.chirp?.[3] ?? 0)).toFixed(1),
          n: sw.rate!.toFixed(1),
          dir,
          rate: k,
          w: (sw.width / 1000).toFixed(0),
        }),
      );
    else if (sw.width != null)
      text.push(t("Sweeps {dir} at {rate} kHz/s, {n} a second: about {w} kHz wide.", { dir, rate: k, n: sw.rate!.toFixed(1), w: (sw.width / 1000).toFixed(0) }));
  }
  if (f.frame != null && f.ofdm == null && !f.sweep) text.push(t("A pattern repeats every {ms} ms (frames or probe sequences).", { ms: (1000 * f.frame).toFixed(f.frame < 0.1 ? 1 : 0) }));
  if (f.pulses != null && chosen?.cls !== "pactor" && !f.sweep) text.push(t("Pulses every {ms} ms ({rate} a second).", { ms: (1000 * f.pulses).toFixed(1), rate: (1 / f.pulses).toFixed(1) }));
  if (a.drift != null) {
    const d = ctx.rf(a.centre + a.drift) - rf;
    text.push(t("Drifting {rate} Hz a second.", { rate: (d > 0 ? "+" : "") + d.toFixed(1) }));
  }
  // where it is
  text.push(...where(rf, rfLo, rfHi, ctx));

  const facts: [string, string][] = [
    [t("Centre"), `${formatKhz(Math.round(rf))} kHz`],
    [t("Width"), a.fills ? `≥ ${hz(width)}` : t("{width} ({w90} holds 90 % of the power)", { width: hz(width), w90: hz(a.bw90) })],
    [t("SNR"), a.fills ? "?" : `${Math.round(a.snr)} dB`],
    [t("Time on"), `${Math.round(100 * a.duty)} %`],
  ];
  if (a.tones.length) facts.push([t("Tones"), a.tones.slice(0, 8).map(([h]) => `${formatKhz(Math.round(ctx.rf(h)))}`).join(", ")]);
  if (a.baudEnv) facts.push([t("Envelope line"), `${a.baudEnv[0].toFixed(2)} Hz (${Math.round(a.baudEnv[1])} dB)`]);
  if (a.baudFreq) facts.push([t("Frequency-change line"), `${a.baudFreq[0].toFixed(2)} Hz (${Math.round(a.baudFreq[1])} dB)`]);
  if (a.period) facts.push([t("Repeats"), `${(1000 * a.period[0]).toFixed(1)} ms (r ${a.period[1].toFixed(2)})`]);
  if (a.envPeriod) facts.push([t("Envelope repeats"), `${(1000 * a.envPeriod[0]).toFixed(1)} ms (r ${a.envPeriod[1].toFixed(2)})`]);
  if (a.shift != null) facts.push([t("Shift"), `${Math.round(a.shift)} Hz`]);
  facts.push([t("Carrier share"), `${Math.round(100 * a.line)} %`]);
  // i18n: peak to average power ratio of the signal
  facts.push([t("Peak to average"), `${(a.papr ?? 0).toFixed(1)} dB`]);
  facts.push([t("Identifier"), top.slice(0, 3).map((x) => `${label(x.cls)} ${Math.round(100 * x.p)} %`).join(", ")]);
  return { title, sure, cls, rf, rfLo, rfHi, text, facts, fills: a.fills };
}

/** Where it is: the band plan's band or segment, the labels near it. */
export function where(rf: number, lo: number, hi: number, ctx: DescCtx): string[] {
  const out: string[] = [];
  const p = ctx.plan;
  if (p) {
    const ham = p.amateur.find((b) => rf >= b.lo && rf <= b.hi);
    const seg = ham?.segments.find((s) => rf >= s.lo && rf <= s.hi);
    const svc = p.services.filter((s) => rf >= s.lo && rf <= s.hi).sort((x, y) => x.hi - x.lo - (y.hi - y.lo))[0];
    if (ham && seg) out.push(t("In the {band} amateur band, its {use} segment.", { band: ham.name, use: useName(seg.use) }));
    else if (ham) out.push(t("In the {band} amateur band.", { band: ham.name }));
    else if (svc) out.push(t("In the {name} band ({service}).", { name: svc.name, service: svc.service }));
  }
  const margin = 500;
  const near = ctx.markers
    .filter((m) => m.hz >= lo - margin && m.hz <= hi + margin)
    .sort((x, y) => Math.abs(x.hz - (lo + hi) / 2) - Math.abs(y.hz - (lo + hi) / 2))
    .slice(0, 3);
  if (near.length)
    out.push(
      t("Listed near it: {list}.", {
        list: near.map((m) => `${m.name} (${formatKhz(m.hz)} kHz${m.emission ? `, ${m.emission.toUpperCase()}` : ""})`).join("; "),
      }),
    );
  return out;
}

/** Where an RF range is in this tuning's audio, or how to tune so that
 * it is: the mode kept when the identifier reads it (USB, LSB, CW), else
 * USB; the passband widened to hold the range with 200 Hz on each side
 * (up to 5 kHz). */
export type AskPlan =
  | { audio: [number, number] }
  | { tune: { dial: number; mode: "usb" | "lsb" | "cw"; lo: number; hi: number } };

export function askPlan(
  s: { mode: string; hz: number; lo: number; hi: number; pitch: number },
  band: { lo: number; hi: number } | null,
  rfLo: number,
  rfHi: number,
): AskPlan {
  const audio = (rf: number) => (s.mode === "lsb" ? s.hz - rf : s.mode === "cw" ? rf - s.hz + s.pitch : rf - s.hz);
  if (band) {
    const [a, b] = [audio(rfLo), audio(rfHi)].sort((x, y) => x - y);
    const lo = Math.max(a, band.lo);
    const hi = Math.min(b, band.hi);
    // mostly inside: what is inside is measured
    if (hi - lo >= 0.8 * (b - a) && hi - lo >= 20) return { audio: [lo, hi] };
  }
  const mode = s.mode === "lsb" || s.mode === "cw" ? (s.mode as "lsb" | "cw") : "usb";
  const centre = (rfLo + rfHi) / 2;
  if (mode === "cw" && rfHi - rfLo <= 800) {
    // the range around the pitch, the CW passband as it is if it holds it
    const half = Math.max((rfHi - rfLo) / 2 + 200, (s.hi - s.lo) / 2);
    return { tune: { dial: Math.round(centre), mode, lo: -Math.round(half), hi: Math.round(half) } };
  }
  // SSB: as wide as needed, at least as wide as it was
  const width = Math.min(5000, Math.max(rfHi - rfLo + 400, s.mode === "usb" || s.mode === "lsb" ? s.hi - s.lo : 2600));
  const lo = 100;
  const hi = Math.round(lo + width);
  const mid = (lo + hi) / 2;
  const ssb = mode === "cw" ? "usb" : mode;
  return ssb === "lsb"
    ? { tune: { dial: Math.round(centre + mid), mode: "lsb", lo: -hi, hi: -lo } }
    : { tune: { dial: Math.round(centre - mid), mode: "usb", lo, hi } };
}
