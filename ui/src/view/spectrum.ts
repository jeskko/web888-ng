// SPDX-License-Identifier: GPL-3.0-or-later
// Spectrum (Canvas 2D) above the waterfall: the newest line (peak of the
// bins under each pixel), a slow average, dB grid, the frequency scale,
// and the receiver's passband and dial marker.

import { FLAG_NEW_GEN, type WfLine } from "../proto.ts";
import { tickLabel, tickStep } from "../fmt.ts";

export const SCALE_PX = 20;

export interface Passband {
  dial: number;
  lo: number;
  hi: number;
}

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

  draw(start: number, span: number, minDb: number, maxDb: number, pb: Passband | null, dpr: number): void {
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

    // dial marker
    if (pb) {
      const xd = Math.round(x(pb.dial)) + 0.5;
      ctx.strokeStyle = "#ff5a5a";
      ctx.lineWidth = dpr;
      ctx.beginPath();
      ctx.moveTo(xd, 0);
      ctx.lineTo(xd, ph);
      ctx.stroke();
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
  }
}
