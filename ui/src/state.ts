// SPDX-License-Identifier: GPL-3.0-or-later
// App state (Preact signals) and the actions that change it. The server's
// `state` message is authoritative; actions update the local copy at once
// (optimistic) and send the request.

import { batch, signal } from "@preact/signals";
import { Player, type PlayerStats } from "./audio/player.ts";
import { clamp, formatKhz } from "./fmt.ts";
import { formatHash, parseHash } from "./hash.ts";
import { Conn, wsUrl, type ConnState } from "./net.ts";
import type { AudioFrame, AudioReq, Limits, Mode, RadioInfo, Spot, State, Status, UserMark, WfLine, WfReq } from "./proto.ts";
import { FLAG_GAP, FLAG_OVERLOAD, wfSpan } from "./proto.ts";
import { addSpots } from "./spots.ts";

export const conn = signal<ConnState>("connecting");
/** The receiver wants its listener password (and we have no login). */
export const needLogin = signal(false);
export const loginError = signal("");
export const connNote = signal("");
export const radio = signal<RadioInfo | null>(null);
export const limits = signal<Limits | null>(null);
export const server = signal("");
export const st = signal<State | null>(null);
export const sessionId = signal<number | null>(null);
/** The session's receiver: granted, or waiting at `position` in the queue. */
export const lease = signal<{ granted: boolean; position: number } | null>(null);
/** Other users who share their frequency. */
export const users = signal<UserMark[]>([]);
/** The server runs skimmers; their spots, oldest first. */
export const spotsAvail = signal(false);
export const spots = signal<Spot[]>([]);
/** The spot list is open (remembered in this browser). */
export const spotsOpen = signal(load("spotsOpen") === "1");
/** The next spots message holds the recent spots (after a hello). */
let spotsFresh = false;
/** A passing message from the server (e.g. listening along ended). */
export const notice = signal("");
export const status = signal<Status | null>(null);
export const latencyMs = signal<number | null>(null);
export const rssi = signal(-200);
/** Time (ms) of the last ADC overload seen in audio frames. */
export const overloadAt = signal(-Infinity);
export const audioGaps = signal(0);
export const player = new Player();
export const audioOn = signal(false);
export const audioStats = signal<PlayerStats | null>(null);
export const audioError = signal("");
export const volume = signal(0.7);
export const muted = signal(false);
/** Waterfall colour range (dB); `auto` follows the noise floor. */
export const range = signal({ min: -130, max: -60, auto: true });

/** Waterfall consumers (canvas views) register here. */
type LineSink = (l: WfLine) => void;
const sinks: LineSink[] = [];
export function onLine(f: LineSink): void {
  sinks.push(f);
}

/** A per-browser setting, when the browser lets us keep one. */
function load(key: string): string | null {
  try {
    return localStorage.getItem(`web888.${key}`);
  } catch {
    return null;
  }
}
function save(key: string, v: string): void {
  try {
    localStorage.setItem(`web888.${key}`, v);
  } catch {
    // not kept: fine
  }
}

let link: Conn | null = null;
/** Hash state to apply once the server said hello. */
let pendingHash = parseHash(location.hash);

/** URL of a server path next to the page. */
function api(path: string): string {
  return location.pathname.replace(/[^/]*$/, "") + path;
}

/** Does the receiver want a password we haven't given? */
async function loginNeeded(): Promise<boolean> {
  try {
    const r = await fetch(api("api/auth"));
    const a = (await r.json()) as { role: string | null; listener_password: boolean };
    return a.listener_password && a.role === null;
  } catch {
    return false; // the server isn't there: the connection retries
  }
}

/** Ask for the password first if the receiver has one, then connect. */
export async function start(): Promise<void> {
  if (await loginNeeded()) askLogin();
  else connect();
}

function askLogin(): void {
  batch(() => {
    needLogin.value = true;
    conn.value = "closed";
    connNote.value = "password needed";
  });
}

export async function login(password: string): Promise<void> {
  loginError.value = "";
  const r = await fetch(api("api/login"), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ password }),
  });
  if (r.ok) {
    needLogin.value = false;
    if (link) link.connect();
    else connect();
    return;
  }
  const body = (await r.json().catch(() => ({}))) as { error?: string };
  loginError.value = body.error ?? `login failed (${r.status})`;
}

export function connect(): void {
  link = new Conn(wsUrl(location), {
    onState(s, why, opened) {
      conn.value = s;
      connNote.value = why ?? "";
      // refused: the password changed, or the login expired
      if (s === "closed" && !opened) {
        void loginNeeded().then((need) => {
          if (need) {
            link?.stop();
            askLogin();
          }
        });
      }
    },
    onMessage(m) {
      switch (m.type) {
        case "hello":
          batch(() => {
            radio.value = m.radio;
            limits.value = m.limits;
            server.value = m.server;
            sessionId.value = m.session;
            spotsAvail.value = !!m.spots;
          });
          if (m.spots) {
            spotsFresh = true;
            link?.send({ type: "spots", on: true });
          }
          break;
        case "state": {
          const { type: _, ...s } = m;
          const first = st.value === null;
          st.value = s;
          if (first) applyHash();
          saveHash();
          break;
        }
        case "status": {
          const { type: _, ...s } = m;
          status.value = s;
          break;
        }
        case "lease":
          lease.value = { granted: m.granted, position: m.position };
          break;
        case "users":
          users.value = m.users;
          break;
        case "spots":
          spots.value = spotsFresh ? m.spots : addSpots(spots.value, m.spots);
          spotsFresh = false;
          break;
        case "busy":
          connNote.value = m.msg;
          break;
        case "error":
          console.warn("web888d:", m.msg);
          showNotice(m.msg);
          break;
      }
    },
    onAudio(f: AudioFrame) {
      rssi.value = f.rssi;
      if (f.flags & FLAG_OVERLOAD) overloadAt.value = performance.now();
      if (f.flags & FLAG_GAP) audioGaps.value++;
      player.push(f.pcm, f.rate);
    },
    onWf(l) {
      for (const s of sinks) s(l);
    },
    onLatency(ms) {
      latencyMs.value = ms;
    },
  });
  link.connect();
  player.onStats = (s) => (audioStats.value = s);
}

export function reconnect(): void {
  link?.connect();
}

function applyHash(): void {
  const h = pendingHash;
  pendingHash = {};
  if (h.mode) setMode(h.mode);
  if (h.hz !== undefined) tune(h.hz);
  const s = st.value;
  if (s && (h.decim !== undefined || h.centerHz !== undefined)) {
    setView({ ...s.wf, decim: h.decim ?? s.wf.decim, center_hz: h.centerHz ?? s.wf.center_hz });
  }
}

let hashTimer: number | undefined;
function saveHash(): void {
  clearTimeout(hashTimer);
  hashTimer = window.setTimeout(() => {
    const s = st.value;
    if (!s) return;
    history.replaceState(null, "", formatHash({ hz: s.hz, mode: s.mode, decim: s.wf.decim, centerHz: s.wf.center_hz }));
    document.title = `${formatKhz(s.hz)} kHz ${s.mode.toUpperCase()} - web888d`;
  }, 300);
}

function update(p: Partial<State>): void {
  if (st.value) st.value = { ...st.value, ...p };
}

let noticeTimer: number | undefined;
function showNotice(msg: string): void {
  notice.value = msg;
  clearTimeout(noticeTimer);
  noticeTimer = window.setTimeout(() => (notice.value = ""), 6000);
}

/** Tune our own receiver (this ends listening along). */
export function tune(hz: number): void {
  const r = radio.value;
  if (r) hz = clamp(hz, r.rf_lo, r.rf_hi);
  hz = Math.round(hz);
  update({ hz, listen: null });
  link?.send({ type: "tune", hz });
}

/** Show our frequency to others and let them listen along, or not. */
export function setShare(on: boolean): void {
  update({ share: on });
  link?.send({ type: "share", on });
}

/** Listen along to another user (while waiting for our own receiver). */
export function listenTo(user: number | null): void {
  link?.send({ type: "listen", user });
}

/** A user marker was clicked: listen along while we have no receiver,
 * else tune to their frequency and mode. */
export function pickUser(m: UserMark): void {
  if (lease.value?.granted) {
    if (st.value?.mode !== m.mode) setMode(m.mode);
    tune(m.hz);
  } else {
    listenTo(m.id);
  }
}

export function showSpots(on: boolean): void {
  spotsOpen.value = on;
  save("spotsOpen", on ? "1" : "0");
}

/** A spot was picked: tune to its skimmer's dial (USB) and bring the
 * signal into view. */
export function pickSpot(s: Spot): void {
  if (st.value?.mode !== "usb") setMode("usb");
  tune(s.dial);
  const v = st.value?.wf;
  const r = radio.value;
  if (!v || !r) return;
  const span = wfSpan(r.adc_hz, v.decim);
  if (Math.abs(s.hz - v.center_hz) > 0.45 * span) setView({ ...v, center_hz: s.dial + 1500 });
}

export function setMode(mode: Mode): void {
  link?.send({ type: "mode", mode });
}

export function setPassband(lo: number, hi: number): void {
  const s = st.value;
  if (!s) return;
  update({ lo, hi });
  link?.send({ type: "mode", mode: s.mode, lo, hi, pitch: s.pitch });
}

export function setAgc(agc: boolean, gain_db?: number): void {
  setAudio({ agc, ...(gain_db !== undefined ? { gain_db } : {}) });
}

/** AGC, noise blanker, noise reduction, squelch. */
export function setAudio(p: AudioReq): void {
  update(p);
  link?.send({ type: "audio", ...p });
}

/** Keep the view inside the tunable range: its edges, not just its
 * centre (a span wider than the range sits in the middle). */
function clampCenter(center: number, decim: number, r: RadioInfo): number {
  const half = wfSpan(r.adc_hz, clamp(decim, r.wf_decim_min, r.wf_decim_max)) / 2;
  if (2 * half >= r.rf_hi - r.rf_lo) return (r.rf_lo + r.rf_hi) / 2;
  return clamp(center, r.rf_lo + half, r.rf_hi - half);
}

/** Throttled: dragging sends at most every 100 ms, and always the last. */
let viewTimer: number | undefined;
let viewPending: WfReq | null = null;
export function setView(w: WfReq): void {
  const r = radio.value;
  if (r) {
    w = {
      ...w,
      decim: clamp(w.decim, r.wf_decim_min, r.wf_decim_max),
      center_hz: clampCenter(w.center_hz, w.decim, r),
    };
  }
  update({ wf: w });
  viewPending = w;
  if (viewTimer === undefined) {
    flushView();
    viewTimer = window.setTimeout(() => {
      viewTimer = undefined;
      if (viewPending) flushView();
    }, 100);
  }
}
function flushView(): void {
  if (viewPending) link?.send({ type: "wf", ...viewPending });
  viewPending = null;
}

export async function startAudio(): Promise<void> {
  try {
    await player.start();
    player.setVolume(muted.value ? 0 : volume.value);
    audioOn.value = true;
    audioError.value = "";
  } catch (e) {
    audioError.value = e instanceof Error ? e.message : String(e);
  }
}

export function setVolume(v: number, mute = muted.value): void {
  volume.value = v;
  muted.value = mute;
  player.setVolume(mute ? 0 : v);
}
