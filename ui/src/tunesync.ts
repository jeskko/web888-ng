// SPDX-License-Identifier: GPL-3.0-or-later
// Tuning faster than the server answers: its `state` for an earlier
// `tune` arrives after we have tuned on, and would put the dial back
// (the phone's wheel sends one per click). Frequencies we sent in the
// last two seconds are remembered; a state that carries one of them
// (not the newest) is an old answer, and the dial keeps the newest.
// Pure, unit-tested.

const KEEP_MS = 2000;

export class TuneSync {
  private sent: { hz: number; at: number }[] = [];

  /** We sent a tune to `hz`. */
  note(hz: number, now: number): void {
    this.sent = this.sent.filter((s) => now - s.at < KEEP_MS);
    this.sent.push({ hz, at: now });
  }

  /** The frequency to show when the server's state says `hz`. */
  settle(hz: number, now: number): number {
    this.sent = this.sent.filter((s) => now - s.at < KEEP_MS);
    const newest = this.sent[this.sent.length - 1];
    if (!newest || hz === newest.hz) return hz;
    return this.sent.some((s) => s.hz === hz) ? newest.hz : hz;
  }
}
