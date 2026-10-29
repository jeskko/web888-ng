// SPDX-License-Identifier: GPL-3.0-or-later
//! Rational resampling by L/M (polyphase), many channels in step: one
//! phase schedule, so each input frame gives every channel the same number
//! of outputs and frames stay whole. `BinRx`'s channels use it at other RX
//! rates than the bins' frame rate, HPSDR's receivers for 48 kS/s from the
//! bins' 60.

use crate::C32;
use crate::fir::{kaiser_len, lowpass};

/// Stopband (dB) and passband edge (fraction of the narrower of the two
/// rates). The stopband starts at 1 - PASS of it: what folds back from
/// the transition band lands above the passband edge, not in it, and the
/// wide transition halves the taps (the cost on the A9).
const ATTEN_DB: f64 = 70.0;
const PASS: f64 = 0.375;

/// The phase schedule and taps of L/M.
pub struct Rational {
    l: usize,
    m: usize,
    /// Phase p's taps, L × the prototype (unity gain), oldest sample's
    /// first: `h[p][j]` weighs the j-th of the history window.
    h: Vec<Vec<f32>>,
    acc: usize,
}

impl Rational {
    pub fn new(l: usize, m: usize) -> Self {
        assert!(l >= 1 && m >= 1);
        // rates relative to the input: up L, filter, down M
        let narrow = (1.0f64).min(l as f64 / m as f64);
        let fu = l as f64; // the upsampled rate, input = 1
        let tw = (1.0 - 2.0 * PASS) * narrow / fu;
        let n = kaiser_len(ATTEN_DB, tw).max(l);
        let fc = 0.5 * narrow / fu;
        let proto = lowpass(n, fc, ATTEN_DB);
        let taps = n.div_ceil(l);
        let h = (0..l)
            .map(|p| {
                (0..taps)
                    .rev()
                    .map(|k| proto.get(p + k * l).map_or(0.0, |&x| (x * l as f64) as f32))
                    .collect()
            })
            .collect();
        Rational { l, m, h, acc: 0 }
    }

    pub fn taps(&self) -> usize {
        self.h[0].len()
    }

    /// After one more input sample: `f` with the taps of each output due,
    /// in order.
    pub fn step(&mut self, mut f: impl FnMut(&[f32])) {
        while self.acc < self.l {
            f(&self.h[self.acc]);
            self.acc += self.m;
        }
        self.acc -= self.l;
    }
}

/// One channel's input history, twice in a row so that the last `n`
/// samples are one slice, `[pos, pos + n)`, oldest first.
#[derive(Clone)]
pub struct History {
    re: Vec<f32>,
    im: Vec<f32>,
    pos: usize,
}

impl History {
    pub fn new(taps: usize) -> Self {
        History {
            re: vec![0.0; 2 * taps],
            im: vec![0.0; 2 * taps],
            pos: 0,
        }
    }

    pub fn push(&mut self, x: C32) {
        let n = self.re.len() / 2;
        self.re[self.pos] = x.re;
        self.re[self.pos + n] = x.re;
        self.im[self.pos] = x.im;
        self.im[self.pos + n] = x.im;
        self.pos = if self.pos + 1 == n { 0 } else { self.pos + 1 };
    }

    /// The window (oldest first) weighed by `h`.
    pub fn dot(&self, h: &[f32]) -> C32 {
        let n = h.len();
        let (re, im) = (
            &self.re[self.pos..self.pos + n],
            &self.im[self.pos..self.pos + n],
        );
        // four partial sums: no dependency chain on one accumulator
        let (mut r, mut i) = ([0f32; 4], [0f32; 4]);
        for ((h, re), im) in h
            .as_chunks::<4>()
            .0
            .iter()
            .zip(re.as_chunks::<4>().0)
            .zip(im.as_chunks::<4>().0)
        {
            for k in 0..4 {
                r[k] += h[k] * re[k];
                i[k] += h[k] * im[k];
            }
        }
        let tail = n / 4 * 4;
        let (mut rt, mut it) = (r.iter().sum::<f32>(), i.iter().sum::<f32>());
        for k in tail..n {
            rt += h[k] * re[k];
            it += h[k] * im[k];
        }
        C32::new(rt, it)
    }
}

/// Frames of `channels` samples resampled by L/M.
pub struct Resampler {
    rs: Rational,
    ch: Vec<History>,
}

impl Resampler {
    pub fn new(l: usize, m: usize, channels: usize) -> Self {
        let rs = Rational::new(l, m);
        let ch = vec![History::new(rs.taps()); channels];
        Resampler { rs, ch }
    }

    pub fn channels(&self) -> usize {
        self.ch.len()
    }

    /// One input frame (a sample per channel); the output frames it
    /// completes are appended to `out`.
    pub fn push(&mut self, x: &[C32], out: &mut Vec<C32>) {
        for (c, &x) in self.ch.iter_mut().zip(x) {
            c.push(x);
        }
        let ch = &self.ch;
        self.rs.step(|h| out.extend(ch.iter().map(|c| c.dot(h))));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_fifths_keeps_frames_whole_and_a_tone_its_level() {
        let mut r = Resampler::new(4, 5, 2);
        let mut out = Vec::new();
        let w = 2.0 * std::f32::consts::PI * 1000.0 / 60e3;
        for k in 0..6000 {
            let z = C32::from_polar(1.0, w * k as f32);
            r.push(&[z, z * 0.5], &mut out);
        }
        // 6000 in, 4800 out, two channels each
        assert_eq!(out.len(), 2 * 4800);
        let tail = &out[2 * 1000..];
        for (c, want) in [(0, 1.0), (1, 0.5)] {
            let rms = (tail
                .iter()
                .skip(c)
                .step_by(2)
                .map(|z| z.norm_sqr())
                .sum::<f32>()
                / (tail.len() / 2) as f32)
                .sqrt();
            assert!((rms - want).abs() < 0.01 * want, "{c}: {rms}");
        }
        // the tone's phase advances 2π·1000/48000 an output sample
        let d = (tail[2] * tail[0].conj()).arg();
        assert!((d - 2.0 * std::f32::consts::PI / 48.0).abs() < 1e-3, "{d}");
    }
}
