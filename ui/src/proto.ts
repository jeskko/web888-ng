// SPDX-License-Identifier: GPL-3.0-or-later
// WebSocket protocol v3, mirroring crates/web/src/proto.rs (the
// reference for programs: doc/api.md; history in doc/m2-web-ui.md §4,
// doc/m3-multi-user.md §8, doc/m4-skimmers.md §5).
// Fixtures shared with the Rust tests: crates/web/tests/proto/.

import { mulawDecode } from "./audio/codec.ts";
import { unpackWf } from "./wfpack.ts";

export const PROTO = 3;

/** "iq": Kiwi protocol clients only (not offered in the UI). "nbfm":
 * narrow-band FM (VHF voice). */
export type Mode = "usb" | "lsb" | "cw" | "am" | "sam" | "sal" | "sar" | "sas" | "iq" | "nbfm";
export type AgcSpeed = "fast" | "medium" | "slow" | "custom";
export const MODES: Mode[] = ["usb", "lsb", "cw", "am", "sam", "sal", "sar", "sas", "iq", "nbfm"];
/** The AM family, in the order the AM button cycles through: envelope,
 * then synchronous (both sidebands, lower, upper, stereo). */
export const AM_MODES: Mode[] = ["am", "sam", "sal", "sar", "sas"];

export function isAm(m: Mode): boolean {
  return AM_MODES.includes(m);
}

export interface WfReq {
  center_hz: number;
  decim: number;
  bins: number;
  fps: number;
  /** Packed lines (FLAG_PACKED) where that makes them smaller. */
  pack?: boolean;
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
  /** A converter's offset: the frequencies to show are these plus it
   * (absent: 0). Spots come with it added. */
  freq_offset_hz?: number;
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
  agc_speed: AgcSpeed;
  /** The custom speed's hang time (ms) and decay (dB/s) (older servers: absent). */
  agc_hang_ms?: number;
  agc_decay_db_s?: number;
  /** Noise blanker threshold (impulse / mean level); 0: off. */
  nb: number;
  /** Noise reduction strength 0..1; 0: off. */
  nr: number;
  /** Automatic notch: steady carriers out. */
  notch: boolean;
  squelch: boolean;
  /** Squelch threshold, channel power (dBFS). */
  squelch_db: number;
  /** Audio sample rate: 12000, or 24000 for passbands to +-10 kHz. */
  audio_rate: number;
  /** How the audio comes: "pcm16", "mulaw" or "opus". */
  codec?: Codec;
  wf: WfReq;
  /** Others see this user's frequency and may listen along. */
  share: boolean;
  /** The user listened along to (the dial follows theirs). */
  listen: number | null;
  /** Callsign or nickname, as others see it. */
  name: string;
  /** Audio frames carry their time. */
  timestamps?: boolean;
}

/** Audio settings to change (the others stay). */
export interface AudioReq {
  agc?: boolean;
  gain_db?: number;
  agc_speed?: AgcSpeed;
  /** With agc_speed "custom": hang 0-5000 ms, decay 1-200 dB/s. */
  agc_hang_ms?: number;
  agc_decay_db_s?: number;
  nb?: number;
  nr?: number;
  notch?: boolean;
  squelch?: boolean;
  squelch_db?: number;
  /** One of the hello's `audio_rates`. */
  audio_rate?: number;
  /** One of the hello's `codecs`. */
  codec?: Codec;
  /** Audio frames carry their time (FLAG_TIME), and `timebase` messages
   * come (protocol 3, feature "timestamps"). */
  timestamps?: boolean;
}

/** Another user: in the user list, and (sharing, with a receiver) a
 * marker on the scale. */
export interface UserMark {
  id: number;
  /** Callsign or nickname (absent: none given). */
  name?: string;
  /** Dial and mode, when they share them. */
  hz?: number;
  mode?: Mode;
  /** They have a receiver (else they wait for one). */
  rx: boolean;
}

/** Longest user name (characters). */
export const NAME_MAX = 8;

/** A skimmer (background decoder): the audio range it decodes, absolute Hz. */
export interface SkimMark {
  mode: string;
  dial: number;
  lo: number;
  hi: number;
  /** It has a channel (else it waits for one). */
  on: boolean;
}

/** A decoded transmission from a skimmer. */
export interface Spot {
  /** Slot start, UTC seconds. */
  t: number;
  mode: string;
  /** "40m", or "" outside the amateur bands. */
  band: string;
  /** The skimmer's dial (USB). */
  dial: number;
  /** RF frequency of the signal (its lowest tone). */
  hz: number;
  snr: number;
  dt: number;
  msg: string;
  /** The sender and its locator, when the message says. */
  call?: string;
  grid?: string;
  /** WSPR: drift (Hz) and transmitter power (dBm). */
  drift?: number;
  dbm?: number;
  /** Possibly a false decode: believed once the call is heard again. */
  uncertain?: boolean;
}

export interface Status {
  adc_overloads: number;
  /** The RF attenuator (dB), when it is in: every listener's signals are
   * this much weaker. */
  rf_atten_db?: number;
  clock_ppm: number;
  audio_drops: number;
  wf_drops: number;
  hw_rate_ok: boolean;
  /** The device's CPU load (percent of all cores) and temperature. */
  cpu?: number;
  temp_c?: number;
}

export type ServerMsg =
  | {
      type: "hello";
      proto: number;
      server: string;
      codec: string;
      audio_rate: number;
      /** The audio rates this session can choose (12000; 24000 where the
       * RX rate allows). */
      audio_rates?: number[];
      /** The codecs this session can choose (none listed: pcm16 only). */
      codecs?: string[];
      radio: RadioInfo;
      limits: Limits;
      session: number;
      class: string;
      /** The server runs skimmers (spots can be asked for). */
      spots?: boolean;
      /** Wide IQ can be asked for (a channelizer's bins). */
      wide?: boolean;
      /** What the server can do (protocol 3; `doc/api.md` §8.1). */
      features?: string[];
    }
  | ({ type: "state" } & State)
  | { type: "lease"; granted: boolean; position: number }
  | { type: "users"; users: UserMark[] }
  | { type: "skimmers"; skimmers: SkimMark[] }
  | { type: "spots"; spots: Spot[] }
  | ({ type: "status" } & Status)
  | { type: "pong"; t: number }
  | { type: "timebase"; source: string; uncertainty?: number; adc_ppm?: number }
  | { type: "busy"; msg: string }
  | { type: "error"; msg: string };

export type ClientMsg =
  | { type: "hello"; proto: number; codecs: string[] }
  | { type: "tune"; hz: number }
  | { type: "mode"; mode: Mode; lo?: number; hi?: number; pitch?: number }
  | ({ type: "audio" } & AudioReq)
  | ({ type: "wf" } & WfReq)
  | { type: "ping"; t: number }
  | { type: "share"; on: boolean }
  | { type: "listen"; user: number | null }
  | { type: "spots"; on: boolean }
  | { type: "name"; name: string }
  | { type: "wide"; on: boolean; hz?: number; bits?: 8 | 16; timestamps?: boolean };

export const KIND_AUDIO = 1;
export const KIND_WF = 2;
/** Wide IQ: a channelizer bin's samples (crates/web/src/wideiq.rs). */
export const KIND_IQ = 3;
export const IQ_HEADER = 32;
/** Wide IQ: 16-bit samples (asked for with bits: 16). */
export const FLAG_I16 = 4;
export const FLAG_OVERLOAD = 1;
/** Audio: samples were lost before this frame. */
export const FLAG_GAP = 2;
/** Audio: two channels, interleaved (left first). */
export const FLAG_STEREO = 4;
/** Audio: G.711 mu-law, a byte a sample. */
export const FLAG_MULAW = 8;
/** Audio: one Opus packet (`packet`), decoded by the page. */
export const FLAG_OPUS = 16;
/** Audio: the UTC (s) of the first sample at the antenna follows the
 * header (an f64); the payload after it. */
export const FLAG_TIME = 32;

/** How the server sends the audio. */
export type Codec = "pcm16" | "mulaw" | "opus";
/** Waterfall: first line of a new tuning/zoom. */
export const FLAG_NEW_GEN = 2;
/** Waterfall: no new samples since the previous line (a repeat). */
export const FLAG_STALE = 4;
/** Waterfall: the levels are packed (wfpack.ts). */
export const FLAG_PACKED = 8;
export const AUDIO_HEADER = 16;
export const WF_HEADER = 40;

export interface AudioFrame {
  kind: typeof KIND_AUDIO;
  flags: number;
  seq: number;
  rate: number;
  /** Channel power before AGC, dBFS. */
  rssi: number;
  /** Mono, or (`stereo`) left and right interleaved; empty for an Opus
   * frame until the page decodes `packet`. */
  pcm: Float32Array;
  stereo: boolean;
  /** Opus: the packet, and the samples it decodes to (both channels'). */
  packet?: Uint8Array;
  samples: number;
  /** The frame's size on the wire (bytes). */
  bytes: number;
  /** UTC (s) of the first sample at the antenna (FLAG_TIME). */
  time?: number;
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
  /** The frame's size on the wire (bytes). */
  bytes: number;
}

export interface IqFrame {
  kind: typeof KIND_IQ;
  flags: number;
  seq: number;
  /** The first sample's frame number. */
  frame: number;
  /** The bin's centre (Hz) and sample rate. */
  centre: number;
  rate: number;
  /** The first sample's UTC (s), when asked for (`timestamps`). */
  time?: number;
  /** I, Q interleaved, scaled (1.0: a full-scale sine). */
  iq: Float32Array;
}

/** Decode a binary frame; null for an unknown kind or a short frame. */
export function parseFrame(buf: ArrayBuffer): AudioFrame | WfLine | IqFrame | null {
  const v = new DataView(buf);
  if (buf.byteLength < 4) return null;
  const kind = v.getUint8(0);
  const flags = v.getUint8(1);
  const n = v.getUint16(2, true);
  if (kind === KIND_AUDIO) {
    let pcm: Float32Array;
    let packet: Uint8Array | undefined;
    const timed = (flags & FLAG_TIME) !== 0;
    if (timed && buf.byteLength < AUDIO_HEADER + 8) return null;
    const time = timed ? v.getFloat64(AUDIO_HEADER, true) : undefined;
    const at = AUDIO_HEADER + (timed ? 8 : 0);
    if (flags & FLAG_OPUS) {
      packet = new Uint8Array(buf, at);
      pcm = new Float32Array(0);
    } else if (flags & FLAG_MULAW) {
      if (buf.byteLength < at + n) return null;
      pcm = mulawDecode(new Uint8Array(buf, at, n));
    } else {
      if (buf.byteLength < at + 2 * n) return null;
      pcm = new Float32Array(n);
      for (let k = 0; k < n; k++) pcm[k] = v.getInt16(at + 2 * k, true) / 32768;
    }
    return {
      kind,
      flags,
      seq: v.getUint32(4, true),
      rate: v.getUint32(8, true),
      rssi: v.getFloat32(12, true),
      pcm,
      stereo: (flags & FLAG_STEREO) !== 0,
      packet,
      samples: n,
      bytes: buf.byteLength,
      time,
    };
  }
  if (kind === KIND_WF) {
    let levels: Uint8Array | null;
    if (flags & FLAG_PACKED) levels = unpackWf(new Uint8Array(buf, WF_HEADER), n);
    else levels = buf.byteLength < WF_HEADER + n ? null : new Uint8Array(buf, WF_HEADER, n);
    if (!levels) return null;
    return {
      kind,
      flags,
      seq: v.getUint32(4, true),
      f0: v.getFloat64(8, true),
      df: v.getFloat64(16, true),
      db0: v.getFloat32(24, true),
      dbStep: v.getFloat32(28, true),
      time: v.getFloat64(32, true),
      levels,
      bytes: buf.byteLength,
    };
  }
  if (kind === KIND_IQ) {
    const i16 = (flags & FLAG_I16) !== 0;
    const timed = (flags & FLAG_TIME) !== 0;
    const at = IQ_HEADER + (timed ? 8 : 0);
    if (buf.byteLength < at + (i16 ? 4 : 2) * n) return null;
    const scale = v.getFloat32(28, true);
    const iq = new Float32Array(2 * n);
    if (i16) for (let k = 0; k < 2 * n; k++) iq[k] = v.getInt16(at + 2 * k, true) * scale;
    else {
      const raw = new Int8Array(buf, at, 2 * n);
      for (let k = 0; k < 2 * n; k++) iq[k] = raw[k] * scale;
    }
    return {
      kind,
      flags,
      seq: v.getUint32(4, true),
      frame: Number(v.getBigUint64(8, true)),
      centre: v.getFloat64(16, true),
      rate: v.getFloat32(24, true),
      ...(timed ? { time: v.getFloat64(IQ_HEADER, true) } : {}),
      iq,
    };
  }
  return null;
}

/** Span (Hz) of a waterfall request. */
export function wfSpan(adcHz: number, decim: number): number {
  return adcHz / decim;
}
