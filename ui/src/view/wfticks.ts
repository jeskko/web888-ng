// SPDX-License-Identifier: GPL-3.0-or-later
// Time marks along the waterfall's left edge: where each line came from
// (its Unix time), a mark at every round interval the lines cross, the
// interval chosen so that marks stay at least `minRows` lines apart (pure,
// unit-tested).

/** Intervals a mark may fall on (s). */
const INTERVALS = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 21600];

export interface Tick {
  /** Line index from the newest (0: the top row). */
  row: number;
  /** The round time crossed (Unix ms). */
  t: number;
  label: string;
}

/** Marks for lines `times` (Unix ms, newest first; NaN: no line). */
export function timeTicks(times: ArrayLike<number>, minRows: number): Tick[] {
  const n = times.length;
  // lines per second over what is there (the rate changes with the view)
  let first = -1;
  let last = -1;
  for (let i = 0; i < n; i++) {
    if (!Number.isFinite(times[i])) continue;
    if (first < 0) first = i;
    last = i;
  }
  if (first < 0 || last <= first) return [];
  const secs = (times[first] - times[last]) / 1000;
  if (!(secs > 0)) return [];
  const perS = (last - first) / secs;
  const iv = INTERVALS.find((s) => s * perS >= minRows) ?? INTERVALS[INTERVALS.length - 1];
  const ms = iv * 1000;
  const out: Tick[] = [];
  let lastRow = -Infinity;
  for (let i = first; i < last; i++) {
    const a = times[i];
    const b = times[i + 1];
    if (!Number.isFinite(a) || !Number.isFinite(b) || b >= a) continue;
    // the line at row i is the first after the round time b < t <= a
    const t = Math.floor(a / ms) * ms;
    if (t <= b) continue;
    if (i - lastRow < minRows / 2) continue;
    lastRow = i;
    out.push({ row: i, t, label: utcLabel(t, iv) });
  }
  return out;
}

/** HH:MM:SS (UTC), or HH:MM for whole minutes. */
export function utcLabel(t: number, ivS: number): string {
  const d = new Date(t);
  const p = (v: number) => String(v).padStart(2, "0");
  const hm = `${p(d.getUTCHours())}:${p(d.getUTCMinutes())}`;
  return ivS >= 60 ? hm : `${hm}:${p(d.getUTCSeconds())}`;
}
