// SPDX-License-Identifier: GPL-3.0-or-later
// The phone layout (`doc/m7-mobile-ui.md`): the waterfall as large as it
// goes, the dial on top, and a dock for the thumb: modes, steps, the
// band picker, a VFO wheel (its centre picks the resolution). Frequency
// entry is a keypad of our own (no on-screen keyboard shoving the page
// about); everything rarely used is in a sheet under "More".

import { useSignal, type Signal } from "@preact/signals";
import type { ComponentChildren } from "preact";
import { useRef } from "preact/hooks";
import { BANDS, bandOf } from "../bands.ts";
import { bandwidths, formatKhz, stepTo } from "../fmt.ts";
import { keypadHz, keypadPress } from "../keypad.ts";
import { isAm, type Mode } from "../proto.ts";
import { Pane } from "../view/pane.tsx";
import {
  audioOn,
  cycleAm,
  goBand,
  muted,
  overloadAt,
  rssi,
  setMode,
  setPassband,
  setVolume,
  setWheelStep,
  spotsOpen,
  startAudio,
  st,
  tuneInView,
  volume,
  wheelStep,
} from "../state.ts";
import { angleDelta, nextWheelStep, takeClicks, wheelLabel, wheelTarget, CLICKS_PER_TURN } from "../wheel.ts";
import { JttyCtl, JttyPanel } from "./jtty.tsx";
import { SpotPanel, SpotsCtl } from "./spots.tsx";
import { AudioCtl, ContrastCtl, DspCtl, LeaseBanner, ShareCtl, StatusBar, UsersCtl, ZoomCtl } from "./controls.tsx";

type SheetName = "keypad" | "bands" | "more" | null;

/** Flat one-colour icons (they take the text colour; the text characters
 * for these are drawn as coloured emoji on iOS). */
const icon = (d: string) => () => (
  <svg class="ico" viewBox="0 0 24 24" aria-hidden="true">
    <path d={d} />
  </svg>
);
export const PlayIcon = icon("M7 4.5v15l13-7.5z");
const PauseIcon = icon("M6 4.5h4v15H6zM14 4.5h4v15h-4z");
const LeftIcon = icon("M16 5v14L5 12z");
const RightIcon = icon("M8 5v14l11-7z");

/** Dial, mode and passband, and a slim signal meter. */
function Top({ open }: { open: (s: SheetName) => void }) {
  const s = st.value;
  const p = rssi.value;
  const pos = Math.min(1, Math.max(0, (p + 140) / 120));
  const over = performance.now() - overloadAt.value < 1000;
  const band = s ? bandOf(s.hz) : null;
  const bw = s ? bandwidths(s.mode).find((b) => b.lo === s.lo && b.hi === s.hi)?.label : undefined;
  return (
    <div class="m-top">
      <button class="m-freq" onClick={() => open("keypad")} aria-label="Enter a frequency">
        {s ? formatKhz(s.hz) : "—"}
        <span class="unit">kHz</span>
      </button>
      <div class="m-info">
        {s && (
          <>
            <span>{s.mode.toUpperCase()}</span>
            {bw && <span>{bw}</span>}
            {band && <span>{band.name} m</span>}
          </>
        )}
        <span class={over ? "lamp lit" : "lamp"} hidden={!over}>
          OVL
        </span>
      </div>
      <div class="m-meter" title="Channel power before AGC (dBFS)">
        <div class="fill" style={{ width: `${(pos * 100).toFixed(1)}%` }} />
      </div>
    </div>
  );
}

/** The VFO wheel: turn it to change the dial by the wheel's step per
 * click; tap its centre to pick the step. */
function Wheel() {
  const box = useRef<HTMLDivElement>(null);
  const ticks = useRef<SVGGElement>(null);
  const turn = useRef({ id: -1, last: 0, rest: 0, spin: 0 });
  const angle = (e: PointerEvent) => {
    const r = box.current!.getBoundingClientRect();
    return Math.atan2(e.clientY - (r.top + r.height / 2), e.clientX - (r.left + r.width / 2));
  };
  const down = (e: PointerEvent) => {
    if ((e.target as Element).closest(".m-cap")) return;
    const t = turn.current;
    t.id = e.pointerId;
    t.last = angle(e);
    t.rest = 0;
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
    e.preventDefault();
  };
  const move = (e: PointerEvent) => {
    const t = turn.current;
    const s = st.value;
    if (e.pointerId !== t.id || !s) return;
    const a = angle(e);
    const d = angleDelta(t.last, a);
    t.last = a;
    t.spin += d;
    ticks.current?.setAttribute("transform", `rotate(${((t.spin * 180) / Math.PI).toFixed(1)} 75 75)`);
    const got = takeClicks(t.rest, d);
    t.rest = got.rest;
    if (got.clicks !== 0) {
      tuneInView(wheelTarget(s.hz, wheelStep.value, got.clicks));
      navigator.vibrate?.(4);
    }
  };
  const end = (e: PointerEvent) => {
    if (e.pointerId === turn.current.id) turn.current.id = -1;
  };
  const marks = Array.from({ length: CLICKS_PER_TURN }, (_, i) => {
    const a = (i / CLICKS_PER_TURN) * 2 * Math.PI;
    const major = i % 6 === 0;
    const r0 = major ? 54 : 60;
    return <line key={i} x1={75 + r0 * Math.sin(a)} y1={75 - r0 * Math.cos(a)} x2={75 + 68 * Math.sin(a)} y2={75 - 68 * Math.cos(a)} class={major ? "major" : ""} />;
  });
  return (
    <div class="m-wheel" ref={box} onPointerDown={down} onPointerMove={move} onPointerUp={end} onPointerCancel={end}>
      <svg viewBox="0 0 150 150" aria-hidden="true">
        <circle cx="75" cy="75" r="72" class="rim" />
        <g ref={ticks}>{marks}</g>
      </svg>
      <button class="m-cap" onClick={() => setWheelStep(nextWheelStep(wheelStep.value))} aria-label="Wheel resolution">
        <small>step</small>
        {wheelLabel(wheelStep.value)}
      </button>
    </div>
  );
}

function Dock({ open }: { open: (s: SheetName) => void }) {
  const s = st.value;
  const step = wheelStep.value;
  const am = s && isAm(s.mode) ? s.mode : null;
  const cycleBw = () => {
    if (!s) return;
    const list = bandwidths(s.mode);
    const i = list.findIndex((b) => b.lo === s.lo && b.hi === s.hi);
    const n = list[(i + 1) % list.length];
    setPassband(n.lo, n.hi);
  };
  const bw = s ? bandwidths(s.mode).find((b) => b.lo === s.lo && b.hi === s.hi)?.label : undefined;
  return (
    <div class="m-dock">
      <div class="m-keys">
        <div class="m-row">
          {(["usb", "lsb", "cw"] as Mode[]).map((m) => (
            <button key={m} class={s?.mode === m ? "on" : ""} onClick={() => setMode(m)}>
              {m.toUpperCase()}
            </button>
          ))}
          <button class={am ? "on" : ""} onClick={cycleAm}>
            {(am ?? "am").toUpperCase()}
          </button>
        </div>
        <div class="m-row">
          <button onClick={() => s && tuneInView(stepTo(s.hz, step, -1))} aria-label="Down one step">
            <LeftIcon />
          </button>
          <button onClick={() => s && tuneInView(stepTo(s.hz, step, 1))} aria-label="Up one step">
            <RightIcon />
          </button>
          <button onClick={cycleBw} title="Passband">
            {bw ?? "BW"}
          </button>
          <button onClick={() => open("bands")}>Bands</button>
        </div>
        <div class="m-row">
          {audioOn.value ? (
            <button
              onClick={() => setVolume(volume.value, !muted.value)}
              aria-label={muted.value ? "Unmute" : "Mute"}
              title={muted.value ? "Unmute" : "Mute"}
            >
              {muted.value ? <PlayIcon /> : <PauseIcon />}
            </button>
          ) : (
            <button class="go" onClick={startAudio} aria-label="Start audio" title="Start audio">
              <PlayIcon />
            </button>
          )}
          <button onClick={() => open("more")}>⋯ More</button>
        </div>
      </div>
      <Wheel />
    </div>
  );
}

function Sheet({ title, close, children }: { title: string; close: () => void; children: ComponentChildren }) {
  return (
    <div class="m-sheet-back" onClick={(e) => e.target === e.currentTarget && close()}>
      <div class="m-sheet" role="dialog" aria-label={title}>
        <div class="m-sheet-head">
          <b>{title}</b>
          <button onClick={close} aria-label="Close">
            ✕
          </button>
        </div>
        <div class="m-sheet-body">{children}</div>
      </div>
    </div>
  );
}

/** Digits in kHz (the keyboard's way) or MHz, no on-screen keyboard. */
function Keypad({ close }: { close: () => void }) {
  const buf: Signal<string> = useSignal("");
  const s = st.value;
  const khz = keypadHz(buf.value, false);
  const mhz = keypadHz(buf.value, true);
  const go = (hz: number | null) => {
    if (hz === null) return;
    tuneInView(hz);
    close();
  };
  const key = (k: string) => (
    <button key={k} class="k" onClick={() => (buf.value = keypadPress(buf.value, k))}>
      {k}
    </button>
  );
  return (
    <Sheet title="Frequency" close={close}>
      <div class="m-entry">
        <span class="num">{buf.value || (s ? formatKhz(s.hz).replace(/\.?0+$/, "") : "")}</span>
        <span class="unit">{buf.value ? "" : "now"}</span>
      </div>
      <div class="m-pad">
        {["1", "2", "3", "4", "5", "6", "7", "8", "9", ".", "0", "⌫"].map(key)}
      </div>
      <div class="m-row">
        <button class="go" disabled={khz === null} onClick={() => go(khz)}>
          {khz === null ? "kHz" : `${formatKhz(khz)} kHz`}
        </button>
        <button disabled={mhz === null || (mhz ?? 0) > 1e9} onClick={() => go(mhz)}>
          {mhz === null ? "MHz" : `${(mhz / 1e6).toFixed(3)} MHz`}
        </button>
      </div>
    </Sheet>
  );
}

function Bands({ close }: { close: () => void }) {
  const here = st.value ? bandOf(st.value.hz) : null;
  return (
    <Sheet title="Band" close={close}>
      <div class="m-bands">
        {BANDS.map((b) => (
          <button
            key={b.name}
            class={here?.name === b.name ? "on" : ""}
            onClick={() => {
              goBand(b);
              close();
            }}
          >
            {b.name} m
          </button>
        ))}
      </div>
    </Sheet>
  );
}

function More({ close }: { close: () => void }) {
  return (
    <Sheet title="More" close={close}>
      <h4>Audio</h4>
      <AudioCtl />
      <h4>Noise</h4>
      <DspCtl />
      <h4>Waterfall</h4>
      <ZoomCtl />
      <ContrastCtl />
      <h4>You</h4>
      <UsersCtl />
      <div class="m-row">
        <ShareCtl />
        <SpotsCtl />
        <JttyCtl />
      </div>
      {spotsOpen.value && <SpotPanel />}
      <JttyPanel />
      <StatusBar />
    </Sheet>
  );
}

/** The whole phone layout; `overlay` is the login, busy and start-audio
 * layer over the view. */
export function MobileApp({ overlay }: { overlay: ComponentChildren }) {
  const sheet = useSignal<SheetName>(null);
  const open = (s: SheetName) => (sheet.value = s);
  const close = () => (sheet.value = null);
  return (
    <div class="m-app">
      <Top open={open} />
      <LeaseBanner />
      <div class="m-view">
        <Pane />
        {overlay}
      </div>
      <Dock open={open} />
      {sheet.value === "keypad" && <Keypad close={close} />}
      {sheet.value === "bands" && <Bands close={close} />}
      {sheet.value === "more" && <More close={close} />}
    </div>
  );
}
