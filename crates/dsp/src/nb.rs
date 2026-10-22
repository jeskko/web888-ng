// SPDX-License-Identifier: GPL-3.0-or-later
//! Noise blanker (`doc/m3-multi-user.md` §5): blanks impulses in a
//! channel's IQ at the RX rate, before the channel filter would smear
//! them into milliseconds of ringing.
//!
//! A sample whose magnitude exceeds `threshold` x the running mean
//! magnitude starts a blank. Samples go through a short delay line, so
//! the blank covers a few samples before the detection (the pulse's
//! rising edge) and some after; the gate closes at once and opens over a
//! few samples. Impulses feed the mean only up to the threshold.

use crate::C32;
use std::collections::VecDeque;

/// Samples blanked before and after a detection, and the ramp length
/// (at 48 kS/s: 83 us, 83 us, 42 us).
const PRE: usize = 4;
const POST: usize = 4;
const RAMP: usize = 2;
/// Running mean time constant (samples): ~10 ms at 48 kS/s.
const MEAN_N: f32 = 480.0;

pub struct NoiseBlanker {
    threshold: f32,
    mean: f32,
    delay: VecDeque<C32>,
    /// Samples still to blank (counted at the output).
    left: usize,
    gate: f32,
}

impl NoiseBlanker {
    /// `threshold`: impulse level relative to the mean magnitude.
    pub fn new(threshold: f32) -> Self {
        NoiseBlanker {
            threshold: threshold.max(1.0),
            mean: 0.0,
            delay: VecDeque::from(vec![C32::new(0.0, 0.0); PRE]),
            left: 0,
            gate: 1.0,
        }
    }

    pub fn set_threshold(&mut self, threshold: f32) {
        self.threshold = threshold.max(1.0);
    }

    pub fn process(&mut self, x: &mut [C32]) {
        let k = 1.0 / MEAN_N;
        for s in x {
            let m = s.norm();
            if self.mean == 0.0 {
                self.mean = m;
            }
            let limit = self.threshold * self.mean;
            if m > limit {
                // blank what's in the delay line, this sample and POST
                // more; the mean rises towards the limit, so a real step
                // up in level gets through within about a millisecond
                self.left = PRE + 1 + POST;
                self.mean += k * (limit - self.mean);
            } else {
                self.mean += k * (m - self.mean);
            }
            self.delay.push_back(*s);
            let out = self.delay.pop_front().expect("delay line");
            let want = if self.left > 0 { 0.0 } else { 1.0 };
            self.left = self.left.saturating_sub(1);
            let step = 1.0 / RAMP as f32;
            self.gate = if want < self.gate {
                // close at once: the pre-window already covers the edge
                want
            } else {
                (self.gate + step).min(want)
            };
            *s = out * self.gate;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blanks_impulses_and_passes_the_rest() {
        let mut nb = NoiseBlanker::new(8.0);
        let n = 4800;
        let mut x: Vec<C32> = (0..n)
            .map(|k| C32::from_polar(0.01, 0.3 * k as f32))
            .collect();
        // an impulse at 3000 (two samples), 100 x the level
        x[3000] = C32::new(1.0, 0.0);
        x[3001] = C32::new(-0.8, 0.3);
        let orig = x.clone();
        nb.process(&mut x);
        // output is delayed by PRE samples
        let out = |k: usize| x[k + PRE];
        // the impulse is gone
        assert!(out(3000).norm() < 1e-6 && out(3001).norm() < 1e-6);
        // the blank starts PRE samples before the impulse
        assert!(out(3000 - PRE).norm() < 1e-6);
        // and the signal is back, unchanged, a little after it
        for k in [2000, 3000 + POST + RAMP + 2, 4000] {
            assert!((out(k) - orig[k]).norm() < 1e-6, "sample {k}");
        }
    }

    #[test]
    fn steady_noise_is_not_blanked() {
        // gaussian-ish noise peaks rarely exceed 4x the mean magnitude
        let mut nb = NoiseBlanker::new(6.0);
        let mut r = 1u32;
        let mut x: Vec<C32> = (0..48_000)
            .map(|_| {
                let mut u = || {
                    r ^= r << 13;
                    r ^= r >> 17;
                    r ^= r << 5;
                    (r as f32 / u32::MAX as f32) - 0.5
                };
                C32::new(u() + u() + u(), u() + u() + u())
            })
            .collect();
        let before: f32 = x.iter().map(|s| s.norm_sqr()).sum();
        nb.process(&mut x);
        let after: f32 = x.iter().map(|s| s.norm_sqr()).sum();
        assert!(after > 0.999 * before, "{after} / {before}");
    }
}
