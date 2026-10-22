// SPDX-License-Identifier: GPL-3.0-or-later
// WebSocket protocol v2, mirroring crates/web/src/proto.rs
// (doc/m2-web-ui.md §4; M3 additions in doc/m3-multi-user.md §8).
// Fixtures shared with the Rust tests: crates/web/tests/proto/.

export const PROTO = 2;

export type Mode = "usb" | "lsb" | "cw" | "am";
export const MODES: Mode[] = ["usb", "lsb", "cw", "am"];

export interface WfReq {
  center_hz: number;
  decim: number;
  bins: number;
  fps: number;
}

export interface RadioInfo {
  backend: string;
  variant: string;
  adc_hz: number;
  rx_rate: number;
  rf_lo: number;
  rf_hi: number;
  wf_decim_min: number;
  wf_decim_max: number;
}

export interface Limits {
  bins_max: number;
  fps_max: number;
}

export interface State {
  hz: number;
  mode: Mode;
  lo: number;
  hi: number;
  pitch: number;
  agc: boolean;
  gain_db: number;
  wf: WfReq;
  /** Others see this user's frequency and may listen along. */
  share: boolean;
  /** The user listened along to (the dial follows theirs). */
  listen: number | null;
}

/** Another user's receiver: a marker on the scale. */
export interface UserMark {
  id: number;
  hz: number;
  mode: Mode;
}

export interface Status {
  adc_overloads: number;
  clock_ppm: number;
  audio_drops: number;
  wf_drops: number;
  hw_rate_ok: boolean;
}

export type ServerMsg =
  | {
      type: "hello";
      proto: number;
      server: string;
      codec: string;
      audio_rate: number;
      radio: RadioInfo;
      limits: Limits;
      session: number;
      class: string;
    }
  | ({ type: "state" } & State)
  | { type: "lease"; granted: boolean; position: number }
  | { type: "users"; users: UserMark[] }
  | ({ type: "status" } & Status)
  | { type: "pong"; t: number }
  | { type: "busy"; msg: string }
  | { type: "error"; msg: string };

export type ClientMsg =
  | { type: "hello"; proto: number; codecs: string[] }
  | { type: "tune"; hz: number }
  | { type: "mode"; mode: Mode; lo?: number; hi?: number; pitch?: number }
  | { type: "audio"; agc?: boolean; gain_db?: number }
  | ({ type: "wf" } & WfReq)
  | { type: "ping"; t: number }
  | { type: "share"; on: boolean }
  | { type: "listen"; user: number | null };

export const KIND_AUDIO = 1;
export const KIND_WF = 2;
export const FLAG_OVERLOAD = 1;
/** Audio: samples were lost before this frame. */
export const FLAG_GAP = 2;
/** Waterfall: first line of a new tuning/zoom. */
export const FLAG_NEW_GEN = 2;
/** Waterfall: no new samples since the previous line (a repeat). */
export const FLAG_STALE = 4;
export const AUDIO_HEADER = 16;
export const WF_HEADER = 40;

export interface AudioFrame {
  kind: typeof KIND_AUDIO;
  flags: number;
  seq: number;
  rate: number;
  /** Channel power before AGC, dBFS. */
  rssi: number;
  pcm: Float32Array;
}

export interface WfLine {
  kind: typeof KIND_WF;
  flags: number;
  seq: number;
  /** Centre frequency of the first bin (Hz). */
  f0: number;
  /** Bin width (Hz). */
  df: number;
  /** Level of value 0 and the step per value (dB). */
  db0: number;
  dbStep: number;
  /** Unix time (ms) of the end of the line's interval. */
  time: number;
  levels: Uint8Array;
}

/** Decode a binary frame; null for an unknown kind or a short frame. */
export function parseFrame(buf: ArrayBuffer): AudioFrame | WfLine | null {
  const v = new DataView(buf);
  if (buf.byteLength < 4) return null;
  const kind = v.getUint8(0);
  const flags = v.getUint8(1);
  const n = v.getUint16(2, true);
  if (kind === KIND_AUDIO) {
    if (buf.byteLength < AUDIO_HEADER + 2 * n) return null;
    const pcm = new Float32Array(n);
    for (let k = 0; k < n; k++) pcm[k] = v.getInt16(AUDIO_HEADER + 2 * k, true) / 32768;
    return {
      kind,
      flags,
      seq: v.getUint32(4, true),
      rate: v.getUint32(8, true),
      rssi: v.getFloat32(12, true),
      pcm,
    };
  }
  if (kind === KIND_WF) {
    if (buf.byteLength < WF_HEADER + n) return null;
    return {
      kind,
      flags,
      seq: v.getUint32(4, true),
      f0: v.getFloat64(8, true),
      df: v.getFloat64(16, true),
      db0: v.getFloat32(24, true),
      dbStep: v.getFloat32(28, true),
      time: v.getFloat64(32, true),
      levels: new Uint8Array(buf, WF_HEADER, n),
    };
  }
  return null;
}

/** Span (Hz) of a waterfall request. */
export function wfSpan(adcHz: number, decim: number): number {
  return adcHz / decim;
}
