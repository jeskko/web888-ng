// SPDX-License-Identifier: GPL-3.0-or-later
// Spectrum (Canvas 2D) above the waterfall: the newest line (peak of the
// bins under each pixel), a slow average, dB grid, the frequency scale,
// the receiver's passband and dial marker, other users' markers, the
// skimmers' ranges, and their latest spots as labels above the scale.

import { FLAG_NEW_GEN, type WfLine } from "../proto.ts";
import { tickLabel, tickStep } from "../fmt.ts";
import { placeLabels } from "../spots.ts";

export const SCALE_PX = 20;

export interface Passband {
  dial: number;
  lo: number;
  hi: number;
  /** The user's name, drawn by the dial marker. */
  name?: string;
}

/** Another user on the scale; `on`: the one listened along to. */
export interface Marker {
  hz: number;
  on: boolean;
  name?: string;
}

/** A skimmer on the scale: the range it decodes. */
export interface SkimBar {
  lo: number;
  hi: number;
  colour: string;
  /** Waiting for a channel: drawn dimmed. */
  waiting: boolean;
}

/** Spot label colours by mode; skimmer bars use the same. */
export function modeColour(mode: string): string {
  return mode === "WSPR" ? "#ffb37f" : mode === "FT8" ? "#7fd1ff" : "#c89bff";
}

/** Height of the skimmer bars on the scale's bottom edge (CSS px). */
export const SKIM_PX = 4;

/** A spot on the scale: its signal's frequency and label. */
export interface SpotLabel<T = unknown> {
  hz: number;
  text: string;
  /** Not FT8: drawn in another colour (WSPR in a third). */
  other: boolean;
  wspr?: boolean;
  item: T;
}

/** Where a spot's label was drawn (device px), for pointer hits. */
export interface SpotHit<T = unknown> {
  x0: number;
  x1: number;
  y0: number;
  y1: number;
  item: T;
}

/** Spot label rows and row height (CSS px). */
const SPOT_ROWS = 3;
const SPOT_ROW_PX = 13;

/** Marker triangle size (CSS px). */
export const MARK_PX = 7;

/** 20th percentile of a line's levels (dB): a robust noise floor. */
export function noiseFloor(l: WfLine): number {
  const h = new Uint32Array(256);
  for (const v of l.levels) h[v]++;
  let acc = 0;
  const want = 0.2 * l.levels.length;
  for (let v = 0; v < 256; v++) {
    acc += h[v];
    if (acc >= want) return l.db0 + v * l.dbStep;
  }
  return l.db0;
}

export class Spectrum {
  private canvas: HTMLCanvasElement;
  private ctx: CanvasRenderingContext2D;
  private last: WfLine | null = null;
  private avg: Float32Array | null = null;
  /** The spot labels of the last draw. */
  spotHits: SpotHit[] = [];

  constructor(canvas: HTMLCanvasElement) {
    this.canvas = canvas;
    this.ctx = canvas.getContext("2d")!;
  }

  push(l: WfLine): void {
    const fresh =
      !this.avg || !this.last || l.flags & FLAG_NEW_GEN || this.avg.length !== l.levels.length || l.f0 !== this.last.f0 || l.df !== this.last.df;
    if (fresh) {
      this.avg = new Float32Array(l.levels.length);
      for (let k = 0; k < l.levels.length; k++) this.avg[k] = l.db0 + l.levels[k] * l.dbStep;
    } else {
      for (let k = 0; k < l.levels.length; k++) this.avg![k] += 0.15 * (l.db0 + l.levels[k] * l.dbStep - this.avg![k]);
    }
    this.last = l;
  }

  /** A small label on a dark box with its top at `y`: starting at `x`,
   * ending there (`left`), or centred on it. */
  private tag(text: string, x: number, y: number, colour: string, dpr: number, left = false, centre = false): void {
    const { ctx } = this;
    ctx.font = `${11 * dpr}px system-ui, sans-serif`;
    ctx.textBaseline = "top";
    const tw = ctx.measureText(text).width;
    const x0 = centre ? x - tw / 2 : left ? x - tw - 6 * dpr : x;
    ctx.fillStyle = "rgba(11, 14, 22, 0.75)";
    ctx.fillRect(x0 - 2 * dpr, y, tw + 4 * dpr, 14 * dpr);
    ctx.fillStyle = colour;
    ctx.fillText(text, x0, y + 2 * dpr);
    ctx.textBaseline = "middle";
    ctx.font = `${10 * dpr}px system-ui, sans-serif`;
  }

  draw(
    start: number,
    span: number,
    minDb: number,
    maxDb: number,
    pb: Passband | null,
    dpr: number,
    marks: Marker[] = [],
    spots: SpotLabel[] = [],
    skims: SkimBar[] = [],
  ): void {
    const { ctx, canvas } = this;
    const w = canvas.width;
    const h = canvas.height;
    const sh = SCALE_PX * dpr;
    const ph = h - sh; // plot height
    // show a little below the waterfall floor and well above its top
    const lo = minDb - 10;
    const hi = maxDb + 20;
    const y = (db: number) => ph - ((db - lo) / (hi - lo)) * ph;
    const x = (f: number) => ((f - start) / span) * w;

    ctx.fillStyle = "#0b0e16";
    ctx.fillRect(0, 0, w, h);

    // dB grid
    ctx.font = `${10 * dpr}px system-ui, sans-serif`;
    ctx.lineWidth = 1;
    ctx.strokeStyle = "#1c2433";
    ctx.fillStyle = "#5d6b82";
    ctx.textBaseline = "middle";
    for (let d = Math.ceil(lo / 10) * 10; d <= hi; d += 10) {
      const yy = Math.round(y(d)) + 0.5;
      ctx.beginPath();
      ctx.moveTo(0, yy);
      ctx.lineTo(w, yy);
      ctx.stroke();
      if (d % 20 === 0) ctx.fillText(`${d}`, 4 * dpr, yy);
    }

    // passband
    if (pb) {
      const x0 = x(pb.dial + pb.lo);
      const x1 = x(pb.dial + pb.hi);
      ctx.fillStyle = "rgba(120, 170, 255, 0.16)";
      ctx.fillRect(x0, 0, Math.max(1, x1 - x0), ph);
    }

    // traces
    const l = this.last;
    if (l && this.avg) {
      const bin = (px: number) => (start + (px / w) * span - l.f0) / l.df;
      const trace = (val: (k: number) => number, style: string, peak: boolean) => {
        ctx.strokeStyle = style;
        ctx.beginPath();
        let pen = false;
        for (let px = 0; px < w; px++) {
          const b0 = Math.round(bin(px));
          const b1 = Math.max(b0, Math.round(bin(px + 1)) - 1);
          if (b1 < 0 || b0 >= l.levels.length) {
            pen = false;
            continue;
          }
          let v = peak ? -Infinity : 0;
          let n = 0;
          for (let k = Math.max(0, b0); k <= Math.min(l.levels.length - 1, b1); k++) {
            v = peak ? Math.max(v, val(k)) : v + val(k);
            n++;
          }
          if (!peak) v /= n;
          const yy = y(v);
          if (pen) ctx.lineTo(px, yy);
          else ctx.moveTo(px, yy);
          pen = true;
        }
        ctx.stroke();
      };
      ctx.lineWidth = dpr;
      const avg = this.avg;
      trace((k) => avg[k], "rgba(255, 210, 90, 0.9)", false);
      trace((k) => l.db0 + l.levels[k] * l.dbStep, "rgba(180, 220, 255, 0.45)", true);
    }

    // spots: a tick each, and labels in rows above the scale where they fit
    this.spotHits = [];
    if (spots.length) {
      ctx.font = `${10 * dpr}px system-ui, sans-serif`;
      ctx.textBaseline = "alphabetic";
      ctx.textAlign = "center";
      const rowH = SPOT_ROW_PX * dpr;
      const items = spots.map((s) => ({ item: s, x: x(s.hz), width: ctx.measureText(s.text).width })).filter((s) => s.x >= 0 && s.x <= w);
      for (const p of placeLabels(items, SPOT_ROWS, 4 * dpr)) {
        const s = p.item;
        const colour = s.wspr ? modeColour("WSPR") : s.other ? modeColour("FT4") : modeColour("FT8");
        ctx.fillStyle = colour;
        ctx.fillRect(Math.round(p.x), ph - 3 * dpr, dpr, 3 * dpr);
        if (p.row < 0) continue;
        const base = ph - 4 * dpr - p.row * rowH;
        const tw = ctx.measureText(s.text).width;
        ctx.fillStyle = "rgba(11, 14, 22, 0.7)";
        ctx.fillRect(p.x - tw / 2 - dpr, base - rowH + 2 * dpr, tw + 2 * dpr, rowH);
        ctx.fillStyle = colour;
        ctx.fillText(s.text, p.x, base - dpr);
        this.spotHits.push({ x0: p.x - tw / 2, x1: p.x + tw / 2, y0: base - rowH + 2 * dpr, y1: base + 2 * dpr, item: s.item });
      }
      ctx.textAlign = "start";
      ctx.textBaseline = "middle";
    }

    // dial marker, the user's name beside it
    if (pb) {
      const xd = Math.round(x(pb.dial)) + 0.5;
      ctx.strokeStyle = "#ff5a5a";
      ctx.lineWidth = dpr;
      ctx.beginPath();
      ctx.moveTo(xd, 0);
      ctx.lineTo(xd, ph);
      ctx.stroke();
      if (pb.name) this.tag(pb.name, xd + 3 * dpr, 2 * dpr, "#ff8a8a", dpr, xd > w - 80 * dpr);
    }

    // other users' names over their markers
    for (const u of marks) {
      const xx = x(u.hz);
      if (u.name && xx >= 0 && xx <= w) this.tag(u.name, xx, 2 * dpr, u.on ? "#ffd25a" : "#5ad17a", dpr, false, true);
    }

    // frequency scale
    ctx.fillStyle = "#121722";
    ctx.fillRect(0, ph, w, sh);
    const step = tickStep(span, w / (90 * dpr));
    ctx.strokeStyle = "#5d6b82";
    ctx.fillStyle = "#aab4c5";
    ctx.textBaseline = "middle";
    ctx.textAlign = "center";
    for (let f = Math.ceil(start / step) * step; f <= start + span; f += step) {
      const xx = Math.round(x(f)) + 0.5;
      ctx.beginPath();
      ctx.moveTo(xx, ph);
      ctx.lineTo(xx, ph + 4 * dpr);
      ctx.stroke();
      ctx.fillText(tickLabel(f, step), xx, ph + sh / 2 + 2 * dpr);
    }
    ctx.textAlign = "start";

    // skimmers: bars on the scale's bottom edge, at least 3 px wide
    const kb = SKIM_PX * dpr;
    for (const k of skims) {
      const x0 = x(k.lo);
      const x1 = x(k.hi);
      if (x1 < 0 || x0 > w) continue;
      const wd = Math.max(3 * dpr, x1 - x0);
      ctx.globalAlpha = k.waiting ? 0.35 : 0.9;
      ctx.fillStyle = k.colour;
      ctx.fillRect((x0 + x1) / 2 - wd / 2, h - kb, wd, kb);
    }
    ctx.globalAlpha = 1;

    // other users: triangles on the scale's top edge
    const m = MARK_PX * dpr;
    for (const u of marks) {
      const xx = x(u.hz);
      if (xx < -m || xx > w + m) continue;
      ctx.fillStyle = u.on ? "#ffd25a" : "#5ad17a";
      ctx.beginPath();
      ctx.moveTo(xx - m / 2, ph);
      ctx.lineTo(xx + m / 2, ph);
      ctx.lineTo(xx, ph + m);
      ctx.closePath();
      ctx.fill();
    }
  }
}
