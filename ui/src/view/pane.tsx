// SPDX-License-Identifier: GPL-3.0-or-later
// Spectrum + waterfall pane: canvases, redraw loop, and the pointer
// interactions: wheel zooms around the cursor, drag pans, click tunes,
// shift+wheel steps the dial.

import { useEffect, useRef } from "preact/hooks";
import { snap, snapFor, stepFor } from "../fmt.ts";
import { wfSpan } from "../proto.ts";
import { limits, onLine, radio, range, setView, st, tune } from "../state.ts";
import { noiseFloor, Spectrum } from "./spectrum.ts";
import { Waterfall } from "./waterfall.ts";

/** Auto colour range: follows the noise floor of recent lines. */
const auto = { min: NaN, max: NaN, n: 0 };

function trackRange(floor: number): void {
  const want = { min: floor - 8, max: floor + 50 };
  if (Number.isNaN(auto.min)) Object.assign(auto, want);
  auto.min += 0.1 * (want.min - auto.min);
  auto.max += 0.1 * (want.max - auto.max);
  // the sliders don't need 20 updates a second
  if (++auto.n % 10 === 0 && range.value.auto) {
    range.value = { min: Math.round(auto.min), max: Math.round(auto.max), auto: true };
  }
}

function currentRange(): { min: number; max: number } {
  const r = range.value;
  return r.auto && !Number.isNaN(auto.min) ? auto : r;
}

export function Pane() {
  const specRef = useRef<HTMLCanvasElement>(null);
  const wfRef = useRef<HTMLCanvasElement>(null);
  const boxRef = useRef<HTMLDivElement>(null);
  const errRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const sc = specRef.current!;
    const wc = wfRef.current!;
    const box = boxRef.current!;
    let wf: Waterfall | null = null;
    try {
      wf = new Waterfall(wc);
    } catch (e) {
      errRef.current!.textContent = `Waterfall unavailable: ${e instanceof Error ? e.message : e}`;
    }
    const spec = new Spectrum(sc);
    let dirty = true;
    let dpr = window.devicePixelRatio || 1;

    onLine((l) => {
      wf?.push(l);
      spec.push(l);
      trackRange(noiseFloor(l));
      dirty = true;
    });

    const view = () => {
      const s = st.value;
      const r = radio.value;
      if (!s || !r) return null;
      const span = wfSpan(r.adc_hz, s.wf.decim);
      return { start: s.wf.center_hz - span / 2, span };
    };

    // ask for about one bin per CSS pixel
    const wantBins = () => {
      const s = st.value;
      const lim = limits.value;
      if (!s || !lim) return;
      const bins = Math.min(lim.bins_max, Math.max(256, 2 ** Math.round(Math.log2(box.clientWidth))));
      if (bins !== s.wf.bins) setView({ ...s.wf, bins });
    };

    const resize = () => {
      dpr = window.devicePixelRatio || 1;
      for (const c of [sc, wc]) {
        c.width = Math.round(c.clientWidth * dpr);
        c.height = Math.round(c.clientHeight * dpr);
      }
      wantBins();
      dirty = true;
    };
    const ro = new ResizeObserver(resize);
    ro.observe(box);
    const unsub = st.subscribe(() => {
      dirty = true;
    });
    const unsubBins = limits.subscribe(() => wantBins());

    let raf = 0;
    const frame = () => {
      raf = requestAnimationFrame(frame);
      if (!dirty) return;
      dirty = false;
      const v = view();
      const s = st.value;
      if (!v || !s) return;
      const { min, max } = currentRange();
      wf?.draw(v.start, v.span, min, max);
      spec.draw(v.start, v.span, min, max, { dial: s.hz, lo: s.lo, hi: s.hi }, dpr);
    };
    raf = requestAnimationFrame(frame);
    const unsubRange = range.subscribe(() => {
      dirty = true;
    });

    // pointer interaction (on both canvases)
    let drag: { x: number; center: number; moved: boolean; id: number } | null = null;
    const freqAt = (clientX: number) => {
      const v = view();
      if (!v) return null;
      const rect = box.getBoundingClientRect();
      return v.start + ((clientX - rect.left) / rect.width) * v.span;
    };
    const down = (e: PointerEvent) => {
      const s = st.value;
      if (!s || e.button !== 0) return;
      drag = { x: e.clientX, center: s.wf.center_hz, moved: false, id: e.pointerId };
      (e.currentTarget as Element).setPointerCapture(e.pointerId);
    };
    const move = (e: PointerEvent) => {
      const s = st.value;
      const v = view();
      if (!drag || !s || !v || e.pointerId !== drag.id) return;
      const dx = e.clientX - drag.x;
      if (!drag.moved && Math.abs(dx) < 4) return;
      drag.moved = true;
      const center = drag.center - (dx / box.clientWidth) * v.span;
      setView({ ...s.wf, center_hz: center });
    };
    const up = (e: PointerEvent) => {
      const s = st.value;
      if (drag && !drag.moved && s) {
        const f = freqAt(e.clientX);
        if (f !== null) tune(snap(f, snapFor(s.mode)));
      }
      drag = null;
    };
    const wheel = (e: WheelEvent) => {
      const s = st.value;
      const r = radio.value;
      const v = view();
      if (!s || !r || !v) return;
      e.preventDefault();
      if (e.shiftKey) {
        const step = stepFor(s.mode);
        tune(s.hz + (e.deltaY < 0 || e.deltaX < 0 ? step : -step));
        return;
      }
      const decim = e.deltaY < 0 ? s.wf.decim * 2 : s.wf.decim / 2;
      if (decim < r.wf_decim_min || decim > r.wf_decim_max) return;
      const rect = box.getBoundingClientRect();
      const t = (e.clientX - rect.left) / rect.width;
      const fc = v.start + t * v.span;
      const span = wfSpan(r.adc_hz, decim);
      setView({ ...s.wf, decim, center_hz: fc - t * span + span / 2 });
    };
    for (const c of [sc, wc]) {
      c.addEventListener("pointerdown", down);
      c.addEventListener("pointermove", move);
      c.addEventListener("pointerup", up);
      c.addEventListener("pointercancel", () => (drag = null));
      c.addEventListener("wheel", wheel, { passive: false });
    }

    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
      unsub();
      unsubBins();
      unsubRange();
    };
  }, []);

  return (
    <div class="pane" ref={boxRef}>
      <canvas class="spectrum" ref={specRef} />
      <canvas class="waterfall" ref={wfRef} />
      <div class="pane-error" ref={errRef} />
    </div>
  );
}
