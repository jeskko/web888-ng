// SPDX-License-Identifier: GPL-3.0-or-later
// The phone layout (`doc/m7-mobile-ui.md`): the waterfall as large as it
// goes, the dial on top, and a dock for the thumb: modes, steps, the
// band picker, a VFO wheel (its centre picks the resolution). Frequency
// entry is a keypad of our own (no on-screen keyboard shoving the page
// about); everything rarely used is in a sheet under "More".

import { useSignal, type Signal } from "@preact/signals";
import { useDialog } from "../a11y.ts";
import type { ComponentChildren } from "preact";
import { useRef } from "preact/hooks";
import { t, tc } from "../i18n.ts";
import { BANDS, bandLabel, bandOf, bandsIn } from "../bands.ts";
import { bandwidths, formatKhz, shownOffset, stepTo } from "../fmt.ts";
import { keypadHz, keypadPress } from "../keypad.ts";
import { isAm, type Mode } from "../proto.ts";
import { Pane } from "../view/pane.tsx";
import {
  audioOn,
  cycleAm,
  goBand,
  muted,
  overloadAt,
  radio,
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
import { CwCtl, CwPanel } from "./cw.tsx";
import { RttyCtl, RttyPanel } from "./rtty.tsx";
import { ImgCtl, ImgPanel } from "./img.tsx";
import { DigiCtl, DigiPanel } from "./digi.tsx";
import { MapCtl, MapPanel } from "./map.tsx";
import { ConfigBody } from "./config.tsx";
import { MemCtl } from "./memories.tsx";
import { InfoCtl, InfoPanel } from "./info.tsx";
import { PropCtl, PropPanel } from "./prop.tsx";
import { AfPanel } from "./afspec.tsx";
import { AudioRecCtl } from "./audiorec.tsx";
import { SpotPanel, SpotsCtl } from "./spots.tsx";
import { AttLamp, AudioCtl, CodecCtl, ContrastCtl, DspCtl, LeaseBanner, LookCtl, StatusBar, UsersCtl, ZoomCtl } from "./controls.tsx";
import { NearDial } from "./neardial.tsx";

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
  const bw = s ? bandwidths(s.mode, s.audio_rate).find((b) => b.lo === s.lo && b.hi === s.hi)?.label : undefined;
  return (
    <div class="m-top">
      <button class="m-freq" onClick={() => open("keypad")} aria-label={t("Enter a frequency")}>
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
          {/* i18n: OVL = overload: the ADC input is too strong */}
          {t("OVL")}
        </span>
        <AttLamp />
      </div>
      <div class="m-meter" title={t("Channel power before AGC (dBFS)")}>
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
    const tr = turn.current;
    tr.id = e.pointerId;
    tr.last = angle(e);
    tr.rest = 0;
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
    e.preventDefault();
  };
  const move = (e: PointerEvent) => {
    const tr = turn.current;
    const s = st.value;
    if (e.pointerId !== tr.id || !s) return;
    const a = angle(e);
    const d = angleDelta(tr.last, a);
    tr.last = a;
    tr.spin += d;
    ticks.current?.setAttribute("transform", `rotate(${((tr.spin * 180) / Math.PI).toFixed(1)} 75 75)`);
    const got = takeClicks(tr.rest, d);
    tr.rest = got.rest;
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
      <button class="m-cap" onClick={() => setWheelStep(nextWheelStep(wheelStep.value))} aria-label={t("Wheel resolution")}>
        {/* i18n: the dial step per click of the VFO wheel, shown small above its value */}
        <small>{tc("wheel", "step")}</small>
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
    const list = bandwidths(s.mode, s.audio_rate);
    const i = list.findIndex((b) => b.lo === s.lo && b.hi === s.hi);
    const n = list[(i + 1) % list.length];
    setPassband(n.lo, n.hi);
  };
  const bw = s ? bandwidths(s.mode, s.audio_rate).find((b) => b.lo === s.lo && b.hi === s.hi)?.label : undefined;
  return (
    <div class="m-dock">
      <div class="m-keys">
        <div class="m-row">
          {(["usb", "lsb", "cw"] as Mode[]).map((m) => (
            <button key={m} class={s?.mode === m ? "on" : ""} aria-pressed={!!(s?.mode === m)} onClick={() => setMode(m)}>
              {m.toUpperCase()}
            </button>
          ))}
          <button class={am ? "on" : ""} aria-pressed={!!(am)} onClick={cycleAm}>
            {(am ?? "am").toUpperCase()}
          </button>
          <button class={s?.mode === "nbfm" ? "on" : ""} aria-pressed={!!(s?.mode === "nbfm")} onClick={() => setMode("nbfm")}>
            FM
          </button>
        </div>
        <div class="m-row">
          <button onClick={() => s && tuneInView(stepTo(s.hz, step, -1))} aria-label={t("Down one step")}>
            <LeftIcon />
          </button>
          <button onClick={() => s && tuneInView(stepTo(s.hz, step, 1))} aria-label={t("Up one step")}>
            <RightIcon />
          </button>
          <button onClick={cycleBw} title={t("Passband")}>
            {bw ?? "BW"}
          </button>
          {/* the amateur bands are on air: not with a converter's offset */}
          {!radio.value?.freq_offset_hz && <button onClick={() => open("bands")}>{t("Bands")}</button>}
        </div>
        <div class="m-row">
          {audioOn.value ? (
            <button
              onClick={() => setVolume(volume.value, !muted.value)}
              aria-label={muted.value ? t("Unmute") : t("Mute")}
              title={muted.value ? t("Unmute") : t("Mute")}
            >
              {muted.value ? <PlayIcon /> : <PauseIcon />}
            </button>
          ) : (
            <button class="go" onClick={startAudio} aria-label={t("Start audio")} title={t("Start audio")}>
              <PlayIcon />
            </button>
          )}
          <button onClick={() => open("more")}>⋯ {t("More")}</button>
        </div>
      </div>
      <Wheel />
    </div>
  );
}

function Sheet({ title, close, children }: { title: string; close: () => void; children: ComponentChildren }) {
  const box = useDialog(true, close);
  return (
    <div class="m-sheet-back" onClick={(e) => e.target === e.currentTarget && close()}>
      <div ref={box} class="m-sheet" role="dialog" aria-modal="true" aria-label={title}>
        <div class="m-sheet-head">
          <b>{title}</b>
          <button onClick={close} aria-label={t("Close")}>
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
    tuneInView(hz - shownOffset());
    close();
  };
  const key = (k: string) => (
    <button key={k} class="k" onClick={() => (buf.value = keypadPress(buf.value, k))}>
      {k}
    </button>
  );
  return (
    <Sheet title={t("Frequency")} close={close}>
      <div class="m-entry">
        <span class="num">{buf.value || (s ? formatKhz(s.hz).replace(/\.?0+$/, "") : "")}</span>
        <span class="unit">{buf.value ? "" : t("now")}</span>
      </div>
      <div class="m-pad">
        {["1", "2", "3", "4", "5", "6", "7", "8", "9", ".", "0", "⌫"].map(key)}
      </div>
      <div class="m-row">
        <button class="go" disabled={khz === null} onClick={() => go(khz)}>
          {khz === null ? "kHz" : `${formatKhz(khz, true)} kHz`}
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
  const r = radio.value;
  const bands = r ? bandsIn(r.rf_lo, r.rf_hi) : BANDS;
  return (
    <Sheet title={t("Band")} close={close}>
      <div class="m-bands">
        {bands.map((b) => (
          <button
            key={b.name}
            class={here?.name === b.name ? "on" : ""} aria-pressed={!!(here?.name === b.name)}
            onClick={() => {
              goBand(b);
              close();
            }}
          >
            {bandLabel(b)}
          </button>
        ))}
      </div>
    </Sheet>
  );
}

function More({ close }: { close: () => void }) {
  return (
    <Sheet title={t("More")} close={close}>
      <h4>{t("Audio")}</h4>
      <AudioCtl />
      <div class="group">
        <CodecCtl />
      </div>
      <AudioRecCtl />
      <h4>{t("Noise")}</h4>
      <DspCtl />
      <h4>{t("Waterfall")}</h4>
      <ZoomCtl />
      <ContrastCtl />
      <LookCtl />
      <h4>{t("Memories")}</h4>
      <MemCtl inline />
      <h3>{t("Configuration")}</h3>
      <ConfigBody />
      <h4>{t("Users")}</h4>
      <UsersCtl />
      <div class="m-panels">
        <SpotsCtl />
        <MapCtl />
        <DigiCtl />
        <CwCtl />
        <RttyCtl />
        <ImgCtl />
        <JttyCtl />
        <PropCtl />
        <InfoCtl />
      </div>
      {spotsOpen.value && <SpotPanel />}
      <CwPanel />
      <RttyPanel />
      <ImgPanel />
      <JttyPanel />
      <DigiPanel />
      <AfPanel />
      <MapPanel />
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
        <NearDial />
        {overlay}
      </div>
      <Dock open={open} />
      {sheet.value === "keypad" && <Keypad close={close} />}
      {sheet.value === "bands" && <Bands close={close} />}
      {sheet.value === "more" && <More close={close} />}
      <InfoPanel />
      <PropPanel />
    </div>
  );
}
