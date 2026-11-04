// SPDX-License-Identifier: GPL-3.0-or-later
// Frequency labels over the spectrum and the band ribbon under it (the
// server's /api/markers and /api/bandplan; crates/markers): their types,
// colours, the ribbon's pieces for a view, and the texts (pure,
// unit-tested).

import { formatKhz } from "./fmt.ts";

export type Tune = "usb" | "lsb" | "cw" | "am";

/** A label: a station or a frequency of activity. */
export interface FreqMarker {
  hz: number;
  /** Where to put the dial if not on `hz`. */
  dial?: number;
  name: string;
  mode: Tune;
  /** The kind of signal ("am", "cw", "rtty", "fax", "ft8"...). */
  emission?: string;
  category?: string;
  note?: string;
  schedule?: string;
  src: string;
}

export interface Segment {
  lo: number;
  hi: number;
  use: string;
  note?: string;
}

export interface AmateurBand {
  name: string;
  lo: number;
  hi: number;
  note?: string;
  segments: Segment[];
}

export interface ServiceBand {
  name: string;
  service: string;
  lo: number;
  hi: number;
  note?: string;
  /** Drawn over its service's band (a maritime sub-band). */
  sub?: boolean;
}

export interface BandPlan {
  region: number;
  title: string;
  amateur: AmateurBand[];
  services: ServiceBand[];
}

/** The label sources, in the order the Labels control lists them. */
export const LABEL_SOURCES: { id: string; name: string; title: string }[] = [
  { id: "eibi", name: "Broadcast", title: "Shortwave broadcasts on the air now (EiBi schedules)" },
  { id: "utility", name: "Utility", title: "Time signals, maritime, aero, weather fax, beacons" },
  { id: "amateur", name: "Amateur", title: "Digital-mode dials and calling frequencies of the IARU region" },
  { id: "own", name: "Station", title: "This receiver's own list (from its admin)" },
];

/** Labels are asked for only when the view is at most this wide (Hz):
 * wider, there is no room to read them. */
export const LABEL_MAX_SPAN = 3_000_000;

/** A label's colour, by what it is. */
export function markerColour(m: FreqMarker): string {
  if (m.src === "own") return "#ffd25a";
  switch (m.category) {
    case "broadcast":
      return "#ff9b8a";
    case "time":
      return "#7fd1ff";
    case "marine":
      return "#5fe0c0";
    case "aero":
      return "#9db8ff";
    case "fax":
    case "rtty":
    case "utility":
      return "#d6a6ff";
    case "beacon":
      return "#c0e070";
    default:
      return m.src === "amateur" ? "#8fe08f" : "#c8d0dc";
  }
}

/** Ribbon colours: amateur segments by use, other services by service. */
const USE_COLOURS: Record<string, string> = {
  cw: "#2d5f9e",
  digi: "#3f7f7f",
  all: "#3e7a3e",
  beacon: "#8a7a2a",
  sat: "#6b4f8f",
  fm: "#5a6a2a",
  other: "#555f6e",
};
const SERVICE_COLOURS: Record<string, string> = {
  broadcast: "#7a3a3a",
  maritime: "#245e58",
  aero: "#33497a",
  time: "#2f6f8f",
  ism: "#7a5a2a",
  cb: "#5a5a5a",
  other: "#454c58",
};
const SUB_COLOUR = "#1a4a45";

/** One drawable piece of the ribbon. */
export interface RibbonPiece {
  lo: number;
  hi: number;
  colour: string;
  /** Written on it where it fits (a band's name). */
  label: string;
  /** Hover text. */
  title: string;
}

const khz = (hz: number) => formatKhz(hz);

/**
 * The ribbon's pieces in `lo..hi` (Hz), back to front: the other services'
 * bands, their sub-bands, then the amateur bands' segments (labelled with
 * the band's name on the first one).
 */
export function ribbon(plan: BandPlan, lo: number, hi: number): RibbonPiece[] {
  const out: RibbonPiece[] = [];
  const inView = (a: number, b: number) => b > lo && a < hi;
  for (const sub of [false, true]) {
    for (const s of plan.services) {
      if (!!s.sub !== sub || !inView(s.lo, s.hi)) continue;
      out.push({
        lo: s.lo,
        hi: s.hi,
        colour: sub ? SUB_COLOUR : (SERVICE_COLOURS[s.service] ?? SERVICE_COLOURS.other),
        label: sub ? "" : s.name,
        title: `${s.name} (${s.service}) ${khz(s.lo)}-${khz(s.hi)} kHz${s.note ? `: ${s.note}` : ""}`,
      });
    }
  }
  for (const b of plan.amateur) {
    if (!inView(b.lo, b.hi)) continue;
    const segs = b.segments.length ? b.segments : [{ lo: b.lo, hi: b.hi, use: "all" }];
    segs.forEach((g, i) => {
      if (!inView(g.lo, g.hi)) return;
      out.push({
        lo: g.lo,
        hi: g.hi,
        colour: USE_COLOURS[g.use] ?? USE_COLOURS.other,
        label: i === 0 ? b.name : "",
        title: `${b.name} ${khz(g.lo)}-${khz(g.hi)} kHz: ${useName(g.use)}${g.note ? `, ${g.note}` : ""}`,
      });
    });
  }
  return out;
}

/** A segment's use in words. */
export function useName(use: string): string {
  return (
    { cw: "CW", digi: "narrow-band digital", all: "all modes", beacon: "beacons", sat: "satellite", fm: "FM" }[use] ?? use
  );
}

/** A label's hover text: frequency, name, mode, schedule, note. */
export function markerTitle(m: FreqMarker): string {
  const kind = m.emission && m.emission !== m.mode ? `${m.emission.toUpperCase()}, ` : "";
  const parts = [`${khz(m.hz)} kHz ${m.name} (${kind}${m.mode.toUpperCase()})`];
  if (m.schedule) parts.push(`${m.schedule} UTC`);
  if (m.note) parts.push(m.note);
  return `${parts.join(" · ")}: click to tune`;
}

/** Where a click on the label tunes: the dial and the mode. */
export function tuneTo(m: FreqMarker): { hz: number; mode: Tune } {
  return { hz: m.dial ?? m.hz, mode: m.mode };
}

/** The label sources remembered in the browser ("" = none chosen). */
export function parseSources(saved: string | null): string[] {
  if (saved === null) return LABEL_SOURCES.map((s) => s.id);
  return saved.split(",").filter((id) => LABEL_SOURCES.some((s) => s.id === id));
}

/** The range to ask for around a view `lo..hi`: half a span more on each
 * side, so small pans need no new request. */
export function fetchRange(lo: number, hi: number): [number, number] {
  const half = (hi - lo) / 2;
  return [lo - half, hi + half];
}

/** The view `lo..hi` lies inside the range fetched before. */
export function covered(lo: number, hi: number, got: [number, number] | null): boolean {
  return !!got && lo >= got[0] && hi <= got[1];
}
