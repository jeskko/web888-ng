// SPDX-License-Identifier: GPL-3.0-or-later
// JS8 decoding in the browser (pure parts, unit-tested): the submodes and
// their periods, the usual frequencies, and the frames from the worker
// (js8/worker.ts, the web888-js8 crate in WebAssembly) joined into
// messages per frequency, as JS8Call's band activity shows them.

/** A frame as the worker posts it. */
export interface Js8Frame {
  /** Audio frequency of tone 0 (Hz above the dial). */
  hz: number;
  dt: number;
  snr: number;
  /** 1: the first frame of a message, 2: the last, 4: fast data. */
  flags: number;
  kind: "heartbeat" | "compound" | "compound directed" | "directed" | "data";
  text: string;
  from: string | null;
  to: string | null;
  grid: string | null;
}

export interface Js8Result {
  frames: Js8Frame[];
}

export type Js8Submode = "normal" | "fast" | "turbo" | "slow";

export const JS8_SUBMODES: { name: Js8Submode; id: number; period: number; collect: number; baud: number }[] = [
  { name: "normal", id: 0, period: 15, collect: 14, baud: 6.25 },
  { name: "fast", id: 1, period: 10, collect: 9.2, baud: 10 },
  { name: "turbo", id: 2, period: 6, collect: 5.4, baud: 20 },
  { name: "slow", id: 3, period: 30, collect: 28.5, baud: 3.125 },
];

/** JS8Call's usual frequencies: USB dial (the signals 500-2500 Hz up). */
export const JS8_DIALS: { name: string; hz: number }[] = [
  { name: "160 m 1.842 MHz", hz: 1_842_000 },
  { name: "80 m 3.578 MHz", hz: 3_578_000 },
  { name: "40 m 7.078 MHz", hz: 7_078_000 },
  { name: "30 m 10.130 MHz", hz: 10_130_000 },
  { name: "20 m 14.078 MHz", hz: 14_078_000 },
  { name: "17 m 18.104 MHz", hz: 18_104_000 },
  { name: "15 m 21.078 MHz", hz: 21_078_000 },
  { name: "12 m 24.922 MHz", hz: 24_922_000 },
  { name: "10 m 28.078 MHz", hz: 28_078_000 },
];

/** A message: frames from one frequency joined. */
export interface Js8Msg {
  id: number;
  sub: Js8Submode;
  /** RF of tone 0 (Hz). */
  rf: number;
  /** UTC seconds of its first and last frames' periods. */
  utc: number;
  last: number;
  snr: number;
  text: string;
  /** Its last frame came (or a frame said it was a single one). */
  done: boolean;
  /** Its first frame wasn't heard. */
  partial: boolean;
  from: string | null;
  to: string | null;
}

/** Messages kept. */
export const JS8_KEEP = 300;
/** A message still open takes frames this many tone spacings away. */
const NEAR_TONES = 4;
/** An open message whose next frame doesn't come in this many periods is
 * closed. */
const STALE_PERIODS = 3;

let nextId = 1;

/** Messages after a period's frames (newest first). `start` is the
 * period's UTC start, `rf` maps an audio frequency to RF. */
export function applyJs8(prev: Js8Msg[], sub: Js8Submode, frames: Js8Frame[], start: number, rf: (hz: number) => number): Js8Msg[] {
  const m = JS8_SUBMODES.find((s) => s.name === sub)!;
  let msgs = prev.map((x) => ({ ...x }));
  for (const x of msgs) if (!x.done && start - x.last > STALE_PERIODS * m.period && x.sub === sub) x.done = true;
  for (const f of [...frames].sort((a, b) => a.hz - b.hz)) {
    const at = rf(f.hz);
    const open = f.flags & 1 ? undefined : msgs.find((x) => !x.done && x.sub === sub && Math.abs(x.rf - at) <= NEAR_TONES * m.baud && x.last < start);
    if (open) {
      Object.assign(open, {
        text: open.text + f.text,
        last: start,
        snr: f.snr,
        rf: at,
        done: (f.flags & 2) !== 0,
        from: open.from ?? f.from,
        to: open.to ?? f.to,
      });
    } else {
      msgs.unshift({
        id: nextId++,
        sub,
        rf: at,
        utc: start,
        last: start,
        snr: f.snr,
        text: f.text,
        done: (f.flags & 2) !== 0,
        partial: (f.flags & 1) === 0,
        from: f.from,
        to: f.to,
      });
    }
  }
  if (msgs.length > JS8_KEEP) msgs = msgs.slice(0, JS8_KEEP);
  return msgs;
}

/** Stations heard (as senders), the latest first: [call, UTC, SNR]. */
export function heard(msgs: Js8Msg[]): { call: string; utc: number; snr: number }[] {
  const by = new Map<string, { call: string; utc: number; snr: number }>();
  for (const x of msgs) {
    if (!x.from || x.from.startsWith("@") || x.from === "<....>") continue;
    const h = by.get(x.from);
    if (!h || h.utc < x.last) by.set(x.from, { call: x.from, utc: x.last, snr: x.snr });
  }
  return [...by.values()].sort((a, b) => b.utc - a.utc);
}
