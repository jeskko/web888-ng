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
  envCv: number;
  syllabic: number;
  /** [audio Hz, dB over the floor], strongest first. */
  tones: [number, number][];
  spacing: number | null;
  line: number;
  lineAt: number;
  sideLo: number;
  sideHi: number;
  /** [Bd, dB the line stands out] */
  baudEnv: [number, number] | null;
  baudFreq: [number, number] | null;
  /** [order, carrier offset Hz, line share] */
  psk: [number, number, number] | null;
  /** [s, correlation] */
  period: [number, number] | null;
  envPeriod: [number, number] | null;
  drift: number | null;
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
  /** Two tones: their spacing (Hz). */
  shift: number | null;
  /** Several tones on one grid: how many, how far apart. */
  mfsk: { n: number; spacing: number } | null;
  /** Symbol rate (Bd). */
  baud: number | null;
  pskOrder: number | null;
  /** Repetition of the signal / pulses (s). */
  frame: number | null;
  pulses: number | null;
  steady: boolean;
  /** Typical dit (s) when keyed. */
  dit: number | null;
}

const near = (x: number | null | undefined, v: number, tol: number) => x != null && Math.abs(x - v) <= tol * v;

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
    a.tones.slice(2).every(([h, d]) => d < Math.min(t0[1], t1[1]) - 6 || (h - t0[0]) * (h - t1[0]) < 0) &&
    a.line < 0.5;
  // speech has no symbols; fading FSK's level moves as speech's, a
  // voice's spectrum can have two peaks: FSK when its tones switch at a
  // rate
  const fskRate = twoShape && a.baudFreq != null;
  const speech = a.line < 0.25 && a.syllabic >= 0.35 && a.envCv >= 0.15 && width >= 600 && !fskRate;
  const two = twoShape && !speech && (a.envCv < 0.25 || fskRate);
  const mfsk = a.spacing != null && a.tones.length >= 3 && a.envCv < 0.25 && !two && !speech ? { n: a.tones.length, spacing: a.spacing } : null;
  const keyed = a.duty > 0.15 && a.duty < 0.9 && a.bursts >= 4 && a.line >= 0.3 && a.onShort >= 0.015;
  const sides = [a.sideLo, a.sideHi].sort((x, y) => x - y);
  const am = a.line >= 0.3 && sides[0] >= 0.005 && sides[0] * 4 >= sides[1] && !keyed;
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
    shift: two ? Math.abs(t0[0] - t1[0]) : null,
    mfsk,
    baud: carrier || am || speech ? null : baud,
    pskOrder: a.psk?.[0] ?? null,
    frame: speech ? null : (a.period?.[0] ?? null),
    pulses: a.envPeriod && a.duty < 0.85 && !keyed ? a.envPeriod[0] : null,
    steady: a.envCv < 0.1 && a.duty >= 0.9 && !keyed && a.line < 0.3,
    dit: keyed ? a.onShort : null,
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

/** The modes the figures fit, most specific first. */
export function matches(f: Features): Match[] {
  const out: Match[] = [];
  const b = f.baud;
  const n = (v: number) => (Math.abs(v - Math.round(v)) < 0.05 ? String(Math.round(v)) : v.toFixed(2));
  if (f.shift != null && b != null) {
    const sh = RTTY_SHIFT.find((s) => near(f.shift, s, 0.12));
    const bd = RTTY_BAUD.find((r) => near(b, r, 0.03));
    if (near(b, 100, 0.03) && near(f.shift, 170, 0.15))
      // i18n: SITOR-B: the maritime FEC teleprinter mode NAVTEX uses; Bd = baud
      out.push({ title: t("SITOR-B / NAVTEX (100 Bd, 170 Hz shift)"), cls: "fsk", exact: true, rate: 100 });
    else if (bd != null && near(b, 50, 0.03) && near(f.shift, 450, 0.1))
      // i18n: the German weather service's teleprinter broadcasts
      out.push({ title: t("RTTY 50 Bd, 450 Hz shift (as the DWD weather broadcasts)"), cls: "fsk", exact: true, rate: 50 });
    else if (bd != null && sh != null) out.push({ title: t("RTTY {baud} Bd, {shift} Hz shift", { baud: n(bd), shift: sh }), cls: "fsk", exact: true, rate: bd });
    else if (sh != null && [110, 150, 300].some((r) => near(b, r, 0.03)))
      out.push({ title: t("ASCII teleprinter, {baud} Bd, {shift} Hz shift", { baud: n(b), shift: Math.round(f.shift) }), cls: "fsk", exact: true });
  }
  if (f.shift != null && near(f.shift, 800, 0.1))
    // i18n: HF fax: weather charts sent by radio; black and white are two frequencies 800 Hz apart
    out.push({ title: t("HF fax (black and white 800 Hz apart)"), cls: "fax", exact: true, rate: null });
  if (f.mfsk && near(f.mfsk.spacing, 250, 0.05) && (b == null || near(b, 125, 0.05) || near(b, 250, 0.05)))
    // i18n: 2G ALE: automatic link establishment, MIL-STD-188-141; 8-FSK = eight tones
    out.push({ title: t("2G ALE (8-FSK, 250 Hz apart, 125 Bd)"), cls: "ale", exact: true, rate: 125 });
  if (b != null && near(b, 1800, 0.01)) out.push({ title: t("HFDL (1800 Bd PSK, aircraft data link)"), cls: "hfdl", exact: true, rate: 1800 });
  if (near(f.frame, 0.10667, 0.03))
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
  if (f.keyed && f.dit) {
    const wpm = Math.round(1.2 / f.dit);
    // i18n: CW = Morse; WPM = words per minute
    out.push({ title: t("CW (Morse), about {wpm} WPM", { wpm }), cls: "cw", exact: wpm >= 5 && wpm <= 60, rate: null });
  }
  if (f.carrier) out.push({ title: t("An unmodulated carrier"), cls: "carrier", exact: true });
  if (f.am) out.push({ title: t("AM (a carrier with both sidebands)"), cls: "am", exact: true });
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

/** What a class expects of the figures: false when they contradict it. */
function fits(c: SigClass, f: Features, a: SigAn): boolean {
  switch (c) {
    case "am":
      return a.line >= 0.3;
    case "carrier":
      return !f.keyed && a.line >= 0.5;
    case "ssb":
      return !f.am && !f.carrier && a.line < 0.5;
    case "cw":
      return a.line >= 0.2;
    case "fsk":
      return f.shift != null || f.mfsk != null || a.envCv < 0.25;
    case "noise":
      return a.snr < 6;
    default:
      return true;
  }
}

/** The headline, how sure, the sentences and the figures. */
export function describe(a: SigAn, ctx: DescCtx): Desc {
  const f = features(a);
  const ms = matches(f);
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
  const fit = best ? fits(best.cls, f, a) : false;
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
  } else if (best) {
    title = top[1] && top[1].p >= 0.15 ? t("{a} or {b}?", { a: label(best.cls), b: label(top[1].cls) }) : t("{mode}?", { mode: label(best.cls) });
    cls = best.cls;
    sure = "unsure";
    if (!fit) text.push(t("The identifier guessed {mode}, which the figures don't bear out.", { mode: label(best.cls) }));
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
  if (f.am) text.push(t("A carrier with sidebands on both sides: AM."));
  else if (f.amOneSide)
    text.push(t("A carrier with sidebands on one side only: AM heard through an SSB filter (tune in AM to hear it), or a carrier beside SSB."));
  if (f.shift != null) text.push(t("It shifts between two tones {shift} Hz apart.", { shift: Math.round(f.shift) }));
  else if (f.mfsk) text.push(t("{n} tones on a {spacing} Hz grid.", { n: f.mfsk.n, spacing: f.mfsk.spacing.toFixed(1) }));
  // the measured rate, unless the mode named has none or another
  const r = chosen?.rate;
  const rateFits = r === undefined || (r != null && f.baud != null && [r, 2 * r, r / 2].some((x) => near(f.baud, x, 0.03)));
  const shown = r ?? f.baud;
  if (shown != null && rateFits) text.push(t("Symbols at {baud} Bd.", { baud: shown >= 100 ? Math.round(shown) : +shown.toFixed(2) }));
  if (f.pskOrder) text.push(t("Phase modulation with {k} phases ({psk}).", { k: f.pskOrder, psk: pskName(f.pskOrder) }));
  if (f.frame != null) text.push(t("A pattern repeats every {ms} ms (frames or probe sequences).", { ms: (1000 * f.frame).toFixed(f.frame < 0.1 ? 1 : 0) }));
  if (f.pulses != null) text.push(t("Pulses every {ms} ms ({rate} a second).", { ms: (1000 * f.pulses).toFixed(1), rate: (1 / f.pulses).toFixed(1) }));
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
  facts.push([t("Carrier share"), `${Math.round(100 * a.line)} %`]);
  facts.push([t("Identifier"), top.slice(0, 3).map((x) => `${label(x.cls)} ${Math.round(100 * x.p)} %`).join(", ")]);
  return { title, sure, cls, rf, rfLo, rfHi, text, facts };
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
