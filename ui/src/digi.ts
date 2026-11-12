// SPDX-License-Identifier: GPL-3.0-or-later
// FT8, FT4 and WSPR decoded in the browser from this receiver's audio
// (pure parts, unit-tested): the modes and their dial frequencies, the
// clocks that put the audio on UTC, the slots cut from it for the decoder
// (digi/worker.ts: the skimmers' decoders in WebAssembly), and the rows
// listed. Nothing decoded here leaves the browser (no PSKReporter, no
// WSPRnet).

export type DigiMode = "ft8" | "ft4" | "wspr";

export interface ModeInfo {
  label: string;
  /** The decoder's number (crates/digi-wasm). */
  id: number;
  /** Slot length and how much of it is decoded, seconds (the skimmers'). */
  period: number;
  collect: number;
  /** USB passband for it (Hz above the dial). */
  passband: { lo: number; hi: number };
  /** Common dial frequencies (USB, Hz). */
  dials: { band: string; hz: number }[];
}

const dials = (list: [string, number][]) => list.map(([band, khz]) => ({ band, hz: Math.round(khz * 1000) }));

export const MODES: Record<DigiMode, ModeInfo> = {
  ft8: {
    label: "FT8",
    id: 0,
    period: 15,
    collect: 14,
    passband: { lo: 100, hi: 3200 },
    dials: dials([
      ["160m", 1840],
      ["80m", 3573],
      ["60m", 5357],
      ["40m", 7074],
      ["30m", 10136],
      ["20m", 14074],
      ["17m", 18100],
      ["15m", 21074],
      ["12m", 24915],
      ["10m", 28074],
      ["6m", 50313],
    ]),
  },
  ft4: {
    label: "FT4",
    id: 1,
    period: 7.5,
    collect: 6.6,
    passband: { lo: 100, hi: 3200 },
    dials: dials([
      ["80m", 3575],
      ["40m", 7047.5],
      ["30m", 10140],
      ["20m", 14080],
      ["17m", 18104],
      ["15m", 21140],
      ["12m", 24919],
      ["10m", 28180],
      ["6m", 50318],
    ]),
  },
  wspr: {
    label: "WSPR",
    id: 2,
    period: 120,
    collect: 114,
    // the signals are within 1500 ± 100 Hz
    passband: { lo: 1250, hi: 1750 },
    dials: dials([
      ["160m", 1836.6],
      ["80m", 3568.6],
      ["60m", 5364.7],
      ["40m", 7038.6],
      ["30m", 10138.7],
      ["20m", 14095.6],
      ["17m", 18104.6],
      ["15m", 21094.6],
      ["12m", 24924.6],
      ["10m", 28124.6],
      ["6m", 50293],
    ]),
  },
};

export const DIGI_RATE = 12_000;

/** The server's clock (UTC seconds) from the waterfall lines' times: a
 * line arrives later than its time by the network and the server's
 * processing, so the largest (server time − arrival) seen is the best
 * estimate of the clocks' offset. It may creep down slowly (the clocks
 * drift), and a run of lines far below it means a clock stepped. */
export class ServerClock {
  private offset: number | null = null;
  private below = 0;

  /** A line of server time `server` arrived at local time `local`. */
  observe(server: number, local: number): void {
    const o = server - local;
    if (this.offset === null || o > this.offset) {
      this.offset = o;
      this.below = 0;
      return;
    }
    if (o < this.offset - 1) {
      // one late line is the network; many in a row are a stepped clock
      if (++this.below >= 20) {
        this.offset = o;
        this.below = 0;
      }
      return;
    }
    this.below = 0;
    this.offset = Math.max(o, this.offset - 1e-4);
  }

  /** UTC now on the server's clock (the local clock until a line came). */
  now(local: number): number {
    return local + (this.offset ?? 0);
  }

  get known(): boolean {
    return this.offset !== null;
  }
}

/** Audio sample indices to UTC (as the skimmers' frame clock): when audio
 * ending at sample `end` arrives, now − end / rate is late by the delay,
 * and the smallest such offset seen is the best estimate. It may creep up
 * slowly (the sample rate is a little off); a jump forward of more than a
 * second is a new time base. */
export class SampleClock {
  private offset: number | null = null;
  private rate: number;
  constructor(rate: number) {
    this.rate = rate;
  }

  observe(end: number, now: number): void {
    const o = now - end / this.rate;
    if (this.offset === null || o >= this.offset + 1) this.offset = o;
    else this.offset = Math.min(o, this.offset + 1e-5);
  }

  utc(index: number): number | null {
    return this.offset === null ? null : this.offset + index / this.rate;
  }

  index(utc: number): number | null {
    return this.offset === null ? null : (utc - this.offset) * this.rate;
  }

  reset(): void {
    this.offset = null;
  }
}

/** Audio of one slot, from its UTC start. */
export interface Slot {
  start: number;
  audio: Float32Array;
}

/** Keeps the recent audio and hands out slots when they are complete
 * enough to decode (a port of the skimmers' `Slotter`, crates/skim). */
export class Slotter {
  private chunks: Float32Array[] = [];
  private len = 0;
  /** Sample index of the first kept sample. */
  private first = 0;
  private nextStart: number | null = null;
  private period: number;
  private collect: number;
  private rate: number;

  constructor(period: number, collect: number, rate = DIGI_RATE) {
    this.period = period;
    this.collect = collect;
    this.rate = rate;
  }

  /** Start over (a gap in the samples, another channel). */
  reset(): void {
    this.chunks = [];
    this.len = 0;
    this.nextStart = null;
  }

  /** The start of the next slot to be handed out, once known. */
  get next(): number | null {
    return this.nextStart;
  }

  /** Append audio whose first sample is index `first`, and return the
   * slots now complete. */
  push(first: number, audio: Float32Array, clock: SampleClock): Slot[] {
    if (this.len === 0) this.first = first;
    else if (first !== this.first + this.len) {
      this.reset();
      this.first = first;
    }
    this.chunks.push(audio);
    this.len += audio.length;
    const out: Slot[] = [];
    const now = clock.utc(this.first + this.len);
    if (now === null) return out;
    const boundary = Math.ceil(now / this.period) * this.period;
    let start = (this.nextStart ??= boundary);
    // more than a slot away: a clock stepped; start at the next boundary
    if (now >= start + this.period + this.collect || start > now + this.period) {
      start = boundary;
      this.nextStart = start;
    }
    if (now >= start + this.collect) {
      this.nextStart = start + this.period;
      const i = clock.index(start);
      if (i !== null) {
        const i0 = Math.round(i) - this.first;
        // whole slots only: listening from the start
        if (i0 >= 0) out.push({ start, audio: this.slice(i0, Math.floor(this.collect * this.rate)) });
      }
    }
    this.trim(Math.floor((this.period + 1) * this.rate));
    return out;
  }

  /** `n` samples from kept sample `from` (fewer at the end). */
  private slice(from: number, n: number): Float32Array {
    const out = new Float32Array(Math.max(0, Math.min(n, this.len - from)));
    let pos = 0; // kept-sample index of the chunk's start
    let w = 0;
    for (const c of this.chunks) {
      const a = Math.max(from - pos, 0);
      const b = Math.min(from + out.length - pos, c.length);
      if (a < b) {
        out.set(c.subarray(a, b), w);
        w += b - a;
      }
      pos += c.length;
      if (w >= out.length) break;
    }
    return out;
  }

  /** Keep the newest `keep` samples (whole chunks). */
  private trim(keep: number): void {
    while (this.chunks.length > 1 && this.len - this.chunks[0].length >= keep) {
      const c = this.chunks.shift()!;
      this.len -= c.length;
      this.first += c.length;
    }
  }
}

/** A decode as the decoder reports it (crates/digi-wasm). */
export interface DigiDecode {
  snr: number;
  dt: number;
  /** Audio frequency above the dial (tone 0; WSPR: the centre), Hz. */
  hz: number;
  text: string;
  /** WSPR: Hz over the transmission. */
  drift?: number;
  /** Possibly a false decode (OSD, an unknown hashed call, -20 dB or
   * weaker). */
  uncertain?: boolean;
}

/** A slot's result. */
export interface DigiResult {
  ms: number;
  cut: boolean;
  decodes: DigiDecode[];
}

/** A decode as listed. */
export interface DigiRow {
  /** UTC of its slot. */
  t: number;
  mode: DigiMode;
  /** RF frequency (USB: dial + audio). */
  rf: number;
  snr: number;
  dt: number;
  text: string;
  drift?: number;
  uncertain?: boolean;
}

/** Rows kept. */
export const DIGI_KEEP = 1000;

export function toRows(r: DigiResult, mode: DigiMode, dial: number, start: number): DigiRow[] {
  return r.decodes.map((d) => ({
    t: start,
    mode,
    rf: dial + d.hz,
    snr: d.snr,
    dt: d.dt,
    text: d.text,
    ...(d.drift !== undefined ? { drift: d.drift } : {}),
    ...(d.uncertain ? { uncertain: true } : {}),
  }));
}

/** How long a slot may take: until the next slot of the mode is due to
 * be decoded, less a margin. */
export function budgetMs(mode: DigiMode): number {
  const m = MODES[mode];
  return 1000 * (m.period - 1);
}
