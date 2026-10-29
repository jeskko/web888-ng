// SPDX-License-Identifier: GPL-3.0-or-later
//! Cutting a band's audio into UTC slots.
//!
//! The hardware numbers its frames but doesn't timestamp them, so a frame
//! clock maps frame numbers to UTC from the arrival times: the host clock
//! when a block arrives, minus the frames' duration, is late by the
//! transfer and scheduling delay, and the smallest such offset seen is
//! the best estimate. It may creep up slowly, for a drifting ADC clock.

use std::collections::VecDeque;

/// Frame numbers to UTC (seconds since the Unix epoch).
pub struct FrameClock {
    rate: f64,
    offset: Option<f64>,
}

/// Allowed creep of the offset per observation (blocks are ~5 ms apart,
/// so ~0.2 ms per second).
const CREEP: f64 = 1e-6;

impl FrameClock {
    pub fn new(rate: f64) -> Self {
        FrameClock { rate, offset: None }
    }

    /// A block ending at frame `end` arrived at `now`.
    pub fn observe(&mut self, end: u64, now: f64) {
        let o = now - end as f64 / self.rate;
        self.offset = Some(match self.offset {
            // A jump forward of more than a second is a new time base
            // (frames restarted, the host clock stepped).
            Some(prev) if o < prev + 1.0 => o.min(prev + CREEP),
            _ => o,
        });
    }

    pub fn utc(&self, frame: f64) -> Option<f64> {
        self.offset.map(|o| o + frame / self.rate)
    }

    pub fn frame(&self, utc: f64) -> Option<f64> {
        self.offset.map(|o| (utc - o) * self.rate)
    }
}

/// Audio of one slot, from its UTC start.
pub struct Slot {
    pub start: f64,
    pub audio: Vec<f32>,
}

/// Keeps the recent audio of a band and hands out slots when they are
/// complete enough to decode.
pub struct Slotter {
    period: f64,
    /// Seconds of each slot to collect before it is handed out.
    collect: f64,
    rate: f64,
    buf: VecDeque<f32>,
    /// Audio sample index (at `rate`) of `buf[0]`.
    first: u64,
    next_start: Option<f64>,
}

impl Slotter {
    pub fn new(period: f64, collect: f64, rate: f64) -> Self {
        Slotter {
            period,
            collect,
            rate,
            buf: VecDeque::new(),
            first: 0,
            next_start: None,
        }
    }

    /// Start over (a gap in the samples or another channel).
    pub fn reset(&mut self) {
        self.buf.clear();
        self.next_start = None;
    }

    /// Append audio whose first sample is index `first` (at `rate`), and
    /// return the slots now complete. `to_utc` maps a sample index to UTC.
    pub fn push(
        &mut self,
        first: u64,
        audio: &[f32],
        to_utc: impl Fn(f64) -> Option<f64>,
        to_index: impl Fn(f64) -> Option<f64>,
    ) -> Vec<Slot> {
        if self.buf.is_empty() {
            self.first = first;
        } else if first != self.first + self.buf.len() as u64 {
            self.reset();
            self.first = first;
        }
        self.buf.extend(audio);
        let mut out = vec![];
        let Some(now) = to_utc((self.first + self.buf.len() as u64) as f64) else {
            return out;
        };
        let boundary = (now / self.period).ceil() * self.period;
        let mut start = *self.next_start.get_or_insert(boundary);
        // More than a slot away: the host clock stepped (NTP on a unit
        // without an RTC steps it by months after boot). Walking there a
        // slot per block would take hours; start at the next boundary.
        if now >= start + self.period + self.collect || start > now + self.period {
            start = boundary;
            self.next_start = Some(start);
        }
        if now >= start + self.collect {
            self.next_start = Some(start + self.period);
            if let Some(i0) = to_index(start) {
                let i0 = i0.round() as i64 - self.first as i64;
                // Only whole slots: the band must have been listening from
                // the start.
                if i0 >= 0 {
                    let n = (self.collect * self.rate) as usize;
                    let audio: Vec<f32> =
                        self.buf.iter().skip(i0 as usize).take(n).copied().collect();
                    out.push(Slot { start, audio });
                }
            }
        }
        // Keep a little more than a slot.
        let keep = ((self.period + 1.0) * self.rate) as usize;
        while self.buf.len() > keep {
            self.buf.pop_front();
            self.first += 1;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_takes_smallest_delay() {
        let mut c = FrameClock::new(48_000.0);
        c.observe(48_000, 1000.0 + 1.0 + 0.020);
        c.observe(96_000, 1000.0 + 2.0 + 0.005);
        c.observe(144_000, 1000.0 + 3.0 + 0.030);
        let t = c.utc(0.0).unwrap();
        assert!((t - 1000.005).abs() < 1e-3, "{t}");
        // A restart of the frame numbers starts over.
        c.observe(48_000, 2000.0 + 1.0);
        assert!((c.utc(0.0).unwrap() - 2000.0).abs() < 1e-6);
    }

    #[test]
    fn cuts_whole_slots_at_utc_boundaries() {
        let rate = 100.0;
        // Sample index i is at UTC 1003.0 + i / rate.
        let to_utc = |i: f64| Some(1003.0 + i / rate);
        let to_index = |t: f64| Some((t - 1003.0) * rate);
        let mut s = Slotter::new(15.0, 14.0, rate);
        let mut slots = vec![];
        let mut i = 0u64;
        while i < 60 * 100 {
            let block: Vec<f32> = (i..i + 50).map(|k| k as f32).collect();
            slots.extend(s.push(i, &block, to_utc, to_index));
            i += 50;
        }
        // Listening from 1003 to 1063: whole slots at 1005, 1020 and 1035
        // (1050 would need audio until 1064).
        let starts: Vec<f64> = slots.iter().map(|s| s.start).collect();
        assert_eq!(starts, [1005.0, 1020.0, 1035.0]);
        // Slot audio starts with the sample at its UTC start.
        assert_eq!(slots[1].audio[0], ((1020.0 - 1003.0) * rate) as f32);
        assert_eq!(slots[1].audio.len(), 1400);
    }

    #[test]
    fn follows_a_clock_step() {
        // 5 s after the start, NTP steps the clock ~7.5 months forward
        // (seen on a unit without an RTC: no slot came for minutes).
        let rate = 100.0;
        let base = std::cell::Cell::new(1003.0);
        let to_utc = |i: f64| Some(base.get() + i / rate);
        let to_index = |t: f64| Some((t - base.get()) * rate);
        let mut s = Slotter::new(15.0, 14.0, rate);
        let mut slots = vec![];
        let mut i = 0u64;
        while i < 60 * 100 {
            if i == 5 * 100 {
                base.set(2e7 + 3.0);
            }
            let block: Vec<f32> = (i..i + 50).map(|k| k as f32).collect();
            slots.extend(s.push(i, &block, to_utc, to_index));
            i += 50;
        }
        // Sample i is now at 2e7 + 3 + i / rate (2e7 + 8 at the step),
        // listening until 2e7 + 63: whole slots from the first boundary
        // after the step (multiples of 15 s: 2e7 + 10, + 25, + 40).
        let starts: Vec<f64> = slots.iter().map(|s| s.start - 2e7).collect();
        assert_eq!(starts, [10.0, 25.0, 40.0]);
    }
}
