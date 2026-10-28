// SPDX-License-Identifier: GPL-3.0-or-later
// Spectrum + waterfall pane: canvases, redraw loop, and the pointer
// interactions: wheel zooms around the cursor, drag pans, click tunes,
// shift+wheel steps the dial, a click on a user marker picks that user,
// a click on a spot label tunes to it. Hovering shows where a click would
// tune: the frequency and the passband there.

import { useEffect, useRef } from "preact/hooks";
import { formatKhz, snap, stepTo } from "../fmt.ts";
import { wfSpan } from "../proto.ts";
import type { Mode, SkimMark, Spot, UserMark } from "../proto.ts";
import { latest, utcTime } from "../spots.ts";
import { followable, lease, limits, myName, onLine, pickSkimmer, pickSpot, pickUser, radio, range, setView, skimmers, snapOf, spots, st, stepOf, tune, users } from "../state.ts";
import { MARK_PX, modeColour, noiseFloor, SCALE_PX, SKIM_PX, Spectrum } from "./spectrum.ts";
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
  const ghostRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const sc = specRef.current!;
    const wc = wfRef.current!;
    const box = boxRef.current!;
    const ghost = ghostRef.current!;
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
    const unsubUsers = users.subscribe(() => {
      dirty = true;
    });
    const unsubName = myName.subscribe(() => {
      dirty = true;
    });
    const unsubSpots = spots.subscribe(() => {
      dirty = true;
    });
    const unsubSkims = skimmers.subscribe(() => {
      dirty = true;
    });

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
      const marks = users.value.filter(followable).map((u) => ({ hz: u.hz, on: u.id === s.listen, name: u.name }));
      const shown = latest(spots.value, Date.now() / 1000).map((p) => ({
        hz: p.hz,
        text: p.call ?? p.msg,
        other: p.mode !== "FT8",
        wspr: p.mode === "WSPR",
        item: p,
      }));
      const bars = skimmers.value.map((k) => ({ lo: k.lo, hi: k.hi, colour: modeColour(k.mode), waiting: !k.on }));
      spec.draw(v.start, v.span, min, max, { dial: s.hz, lo: s.lo, hi: s.hi, name: myName.value }, dpr, marks, shown, bars);
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
    /** Where a click at `clientX` would tune: the passband there (none:
     * hidden). */
    let hoverX: number | null = null;
    const showGhost = (clientX: number | null) => {
      hoverX = clientX;
      const s = st.value;
      const v = view();
      const f = clientX === null ? null : freqAt(clientX);
      if (f === null || !s || !v) {
        ghost.style.display = "none";
        return;
      }
      const hz = snap(f, snapOf(s.mode));
      const pct = (x: number) => ((x - v.start) / v.span) * 100;
      const x0 = pct(hz + s.lo);
      const x1 = pct(hz + s.hi);
      ghost.style.display = "";
      ghost.style.left = `${x0}%`;
      // at least a line wide when zoomed out
      ghost.style.width = `max(1px, ${x1 - x0}%)`;
      const label = ghost.firstElementChild as HTMLElement;
      label.textContent = formatKhz(hz);
      // the label sits on the side with room
      label.classList.toggle("left", (x0 + x1) / 2 > 70);
    };
    const unsubGhost = st.subscribe(() => {
      if (hoverX !== null) showGhost(hoverX);
    });
    /** The user marker under a pointer on the spectrum's scale, if any. */
    const markAt = (e: PointerEvent): (UserMark & { hz: number; mode: Mode }) | null => {
      if (e.currentTarget !== sc) return null;
      const rect = sc.getBoundingClientRect();
      const y = e.clientY - rect.top;
      const v = view();
      if (!v || y < rect.height - SCALE_PX) return null;
      let best: (UserMark & { hz: number; mode: Mode }) | null = null;
      let bestPx = MARK_PX;
      for (const u of users.value.filter(followable)) {
        const px = Math.abs(((u.hz - v.start) / v.span) * rect.width - (e.clientX - rect.left));
        if (px <= bestPx) {
          best = u;
          bestPx = px;
        }
      }
      return best;
    };
    /** The skimmer bar under a pointer on the scale's bottom edge, if any. */
    const skimAt = (e: PointerEvent): SkimMark | null => {
      if (e.currentTarget !== sc) return null;
      const rect = sc.getBoundingClientRect();
      const v = view();
      if (!v || e.clientY - rect.top < rect.height - SKIM_PX - 6) return null;
      const px = e.clientX - rect.left;
      const at = (f: number) => ((f - v.start) / v.span) * rect.width;
      // narrow bars (wide spans) are hit within MARK_PX / 2 of their middle
      return (
        skimmers.value.find((k) => {
          const x0 = at(k.lo);
          const x1 = at(k.hi);
          const pad = Math.max(0, (MARK_PX - (x1 - x0)) / 2);
          return px >= x0 - pad && px <= x1 + pad;
        }) ?? null
      );
    };
    /** The spot label under a pointer on the spectrum, if any. */
    const spotAt = (e: PointerEvent): Spot | null => {
      if (e.currentTarget !== sc) return null;
      const rect = sc.getBoundingClientRect();
      const px = (e.clientX - rect.left) * dpr;
      const py = (e.clientY - rect.top) * dpr;
      const h = spec.spotHits.find((h) => px >= h.x0 && px <= h.x1 && py >= h.y0 && py <= h.y1);
      return h ? (h.item as Spot) : null;
    };
    const down = (e: PointerEvent) => {
      const s = st.value;
      if (!s || e.button !== 0) return;
      const k = skimAt(e);
      if (k) {
        pickSkimmer(k);
        return;
      }
      const m = markAt(e);
      if (m) {
        pickUser(m);
        return;
      }
      const p = spotAt(e);
      if (p) {
        pickSpot(p);
        return;
      }
      drag = { x: e.clientX, center: s.wf.center_hz, moved: false, id: e.pointerId };
      (e.currentTarget as Element).setPointerCapture(e.pointerId);
    };
    const move = (e: PointerEvent) => {
      if (!drag) {
        const k = skimAt(e);
        const m = k ? null : markAt(e);
        const p = k || m ? null : spotAt(e);
        const own = lease.value?.granted;
        sc.title = k
          ? `${k.mode} skimmer, dial ${formatKhz(k.dial)} kHz${k.on ? "" : " (waiting for a channel)"}: click to tune there`
          : m
            ? `${m.name ? `${m.name}: ` : ""}${formatKhz(m.hz)} kHz ${m.mode.toUpperCase()}: click to ${own ? "tune there" : "listen along"}`
            : p
              ? `${utcTime(p.t)} ${p.mode} ${formatKhz(p.hz)} kHz ${p.snr} dB: ${p.msg}`
              : "";
        sc.style.cursor = k || m || p ? "pointer" : "";
        showGhost(k || m || p ? null : e.clientX);
      }
      const s = st.value;
      const v = view();
      if (!drag || !s || !v || e.pointerId !== drag.id) return;
      const dx = e.clientX - drag.x;
      if (!drag.moved && Math.abs(dx) < 4) return;
      drag.moved = true;
      showGhost(null);
      const center = drag.center - (dx / box.clientWidth) * v.span;
      setView({ ...s.wf, center_hz: center });
    };
    const up = (e: PointerEvent) => {
      const s = st.value;
      if (drag && !drag.moved && s) {
        const f = freqAt(e.clientX);
        if (f !== null) tune(snap(f, snapOf(s.mode)));
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
        tune(stepTo(s.hz, stepOf(s.mode), e.deltaY < 0 || e.deltaX < 0 ? 1 : -1));
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
      c.addEventListener("pointerleave", () => showGhost(null));
      c.addEventListener("wheel", wheel, { passive: false });
    }

    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
      unsub();
      unsubBins();
      unsubUsers();
      unsubName();
      unsubSpots();
      unsubSkims();
      unsubRange();
      unsubGhost();
    };
  }, []);

  return (
    <div class="pane" ref={boxRef}>
      <canvas class="spectrum" ref={specRef} />
      <canvas class="waterfall" ref={wfRef} />
      <div class="pane-error" ref={errRef} />
      <div class="ghost" ref={ghostRef} style={{ display: "none" }}>
        <span />
      </div>
    </div>
  );
}
