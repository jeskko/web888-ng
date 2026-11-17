// SPDX-License-Identifier: GPL-3.0-or-later
// The audio (AF) spectrum panel (src/view/afspec.ts): the audio as it
// arrives, a spectrum on top and its waterfall below, in the waterfall's
// palette, the passband shaded. Mono modes show 0 to half the audio
// rate; I/Q shows both sides of the dial. Hovering reads the frequency.

import { useEffect, useRef } from "preact/hooks";
import { AfAnalyzer, type AfLine } from "../view/afspec.ts";
import { colormap } from "../view/colormap.ts";
import { afOn, palette, setAf, st, tapAudio } from "../state.ts";
import type { Mode } from "../proto.ts";

/** Height of the spectrum above the waterfall (CSS px). */
const SPEC_PX = 70;
const AXIS_PX = 14;

/** The passband in audio Hz, as the spectrum's axis has it. */
export function audioPassband(mode: Mode, lo: number, hi: number): [number, number] {
  if (mode === "lsb") return [-hi, -lo];
  if (mode === "usb" || mode === "cw" || mode === "iq") return [lo, hi];
  // AM, FM: the audio is the modulation, up to the passband's edge
  return [0, Math.max(Math.abs(lo), Math.abs(hi))];
}

export function AfPanel() {
  const ref = useRef<HTMLCanvasElement>(null);
  const tip = useRef<HTMLSpanElement>(null);
  const on = afOn.value;
  useEffect(() => {
    if (!on) return;
    const c = ref.current!;
    const ctx = c.getContext("2d")!;
    const an = new AfAnalyzer();
    let last: AfLine | null = null;
    let avg: Float32Array | null = null;
    let floor = NaN;
    let dpr = 1;
    /** The waterfall's rows as pixels (newest at `head`), drawn once a
     * frame when lines came. */
    let ring: ImageData | null = null;
    let head = 0;
    let pending: AfLine[] = [];
    const size = () => {
      dpr = window.devicePixelRatio || 1;
      c.width = Math.round(c.clientWidth * dpr);
      c.height = Math.round(c.clientHeight * dpr);
      const rows = Math.max(1, c.height - (SPEC_PX + AXIS_PX) * dpr);
      ring = ctx.createImageData(c.width, rows);
      head = 0;
    };
    size();
    const ro = new ResizeObserver(size);
    ro.observe(c);
    /** A line into the ring and the running average (no drawing). */
    const take = (l: AfLine) => {
      const n = l.db.length;
      const sorted = Float32Array.from(l.db).sort();
      const f20 = sorted[Math.floor(0.2 * n)];
      floor = Number.isNaN(floor) ? f20 : floor + 0.05 * (f20 - floor);
      const lo = floor - 6;
      const hi = floor + 60;
      if (!avg || avg.length !== n || last?.f0 !== l.f0) avg = Float32Array.from(l.db);
      else for (let i = 0; i < n; i++) avg[i] += 0.3 * (l.db[i] - avg[i]);
      last = l;
      if (!ring) return;
      const w = ring.width;
      head = (head + ring.height - 1) % ring.height;
      const lut = colormap(palette.value);
      const row = ring.data;
      const o = head * w * 4;
      for (let px = 0; px < w; px++) {
        const k = Math.min(n - 1, Math.floor((px / w) * n));
        const t = Math.max(0, Math.min(255, Math.round(((l.db[k] - lo) / (hi - lo)) * 255)));
        row[o + 4 * px] = lut[4 * t];
        row[o + 4 * px + 1] = lut[4 * t + 1];
        row[o + 4 * px + 2] = lut[4 * t + 2];
        row[o + 4 * px + 3] = 255;
      }
    };
    const draw = () => {
      const l = last;
      if (!l || !avg || !ring) return;
      const s = st.value;
      const w = c.width;
      const sh = SPEC_PX * dpr;
      const ah = AXIS_PX * dpr;
      const wy = sh + ah;
      const n = avg.length;
      const lo = floor - 6;
      const hi = floor + 60;
      const x = (f: number) => ((f - l.f0) / (l.f1 - l.f0)) * w;
      // the ring from its newest row down, in two pieces
      const rows = ring.height;
      ctx.putImageData(ring, 0, wy - head, 0, head, w, rows - head);
      if (head > 0) ctx.putImageData(ring, 0, wy + rows - head, 0, 0, w, head);
      ctx.fillStyle = "#0b0e16";
      ctx.fillRect(0, 0, w, wy);
      if (s) {
        const [p0, p1] = audioPassband(s.mode, s.lo, s.hi);
        ctx.fillStyle = "rgba(120, 170, 255, 0.16)";
        ctx.fillRect(x(p0), 0, Math.max(dpr, x(p1) - x(p0)), sh);
      }
      ctx.strokeStyle = "rgba(255, 210, 90, 0.9)";
      ctx.lineWidth = dpr;
      ctx.beginPath();
      for (let i = 0; i < n; i++) {
        const xx = ((i + 0.5) / n) * w;
        const yy = Math.max(0, Math.min(sh, sh - ((avg[i] - lo) / (hi - lo)) * sh));
        if (i) ctx.lineTo(xx, yy);
        else ctx.moveTo(xx, yy);
      }
      ctx.stroke();
      // the axis: a mark every 500 Hz (1 or 2 kHz when wide), labels where they fit
      ctx.fillStyle = "#121722";
      ctx.fillRect(0, sh, w, ah);
      ctx.fillStyle = "#aab4c5";
      ctx.font = `${9 * dpr}px system-ui, sans-serif`;
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      const span = l.f1 - l.f0;
      const step = span > 15000 ? 2000 : span > 7000 ? 1000 : 500;
      for (let f = Math.ceil(l.f0 / step) * step; f <= l.f1; f += step) {
        const xx = Math.round(x(f)) + 0.5;
        ctx.fillRect(xx, sh, dpr, 3 * dpr);
        if (xx > 12 * dpr && xx < w - 12 * dpr) ctx.fillText(f % 1000 ? `${f / 1000}` : `${f / 1000}k`, xx, sh + ah / 2 + dpr);
      }
      ctx.textAlign = "start";
    };
    const stop = tapAudio((f) => {
      pending.push(...an.push(f.pcm, f.rate, f.stereo, st.value?.mode === "iq"));
    });
    let raf = 0;
    const frame = () => {
      raf = requestAnimationFrame(frame);
      if (!pending.length) return;
      const got = pending;
      pending = [];
      for (const l of got) take(l);
      draw();
    };
    raf = requestAnimationFrame(frame);
    // what is under the pointer
    const move = (e: PointerEvent) => {
      const l = last;
      if (!l || !tip.current) return;
      const r = c.getBoundingClientRect();
      const f = l.f0 + ((e.clientX - r.left) / r.width) * (l.f1 - l.f0);
      tip.current.textContent = `${Math.round(f)} Hz`;
    };
    c.addEventListener("pointermove", move);
    return () => {
      stop();
      cancelAnimationFrame(raf);
      ro.disconnect();
      c.removeEventListener("pointermove", move);
    };
  }, [on]);
  if (!on) return null;
  return (
    <aside class="spots afspec">
      <div class="spots-bar">
        <strong>Audio spectrum</strong>
        <span class="dim num" ref={tip} />
        <button class="close" onClick={() => setAf(false)} title="Close">
          ✕
        </button>
      </div>
      <canvas class="afspec-canvas" ref={ref} />
      <p class="dim">The audio as it arrives (before this page's volume), ~23 Hz a point at 12 kS/s; the passband shaded. In IQ, both sides of the dial.</p>
    </aside>
  );
}
