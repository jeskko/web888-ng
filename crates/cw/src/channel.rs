// SPDX-License-Identifier: GPL-3.0-or-later
//! One signal's channel: the input (real audio at [`AUDIO_RATE`], or
//! complex IQ at any rate that is a multiple of [`ENV_FS`], such as a
//! 60 kS/s channelizer bin) mixed down from the signal's frequency and
//! decimated to complex [`ENV_FS`] (±210 Hz pass; with real input the
//! image at −2f is stopped too).

use crate::decode::ENV_FS;
use crate::dsp;
use num_complex::{Complex32, Complex64};

/// The audio input rate.
pub const AUDIO_RATE: f64 = 12_000.0;
/// Taps at 12 kS/s; scaled with the input rate (the same transition band).
const TAPS_12K: usize = 289;

pub struct Channel {
    /// Frequency mixed to 0 (Hz).
    pub hz: f64,
    rate: f64,
    decim: usize,
    taps: Vec<f32>,
    /// The last taps' worth of mixed samples (a ring, doubled for
    /// contiguous reads).
    ring: Vec<Complex32>,
    pos: usize,
    /// The mixer: a rotator, renormalised now and then.
    rot: Complex64,
    step: Complex64,
    count: usize,
    since_norm: usize,
}

impl Channel {
    /// A channel of 12 kS/s audio.
    pub fn new(hz: f64) -> Self {
        Self::with_rate(hz, AUDIO_RATE)
    }

    /// A channel of input at `rate` (a multiple of [`ENV_FS`]).
    pub fn with_rate(hz: f64, rate: f64) -> Self {
        let decim = (rate / ENV_FS).round() as usize;
        let n = ((TAPS_12K as f64 * rate / AUDIO_RATE) as usize) | 1;
        let a = -2.0 * std::f64::consts::PI * hz / rate;
        Channel {
            hz,
            rate,
            decim,
            taps: dsp::firwin(n, 210.0, rate),
            ring: vec![Complex32::new(0.0, 0.0); 2 * n],
            pos: 0,
            rot: Complex64::new(1.0, 0.0),
            step: Complex64::new(a.cos(), a.sin()),
            count: 0,
            since_norm: 0,
        }
    }

    /// The delay of the output behind the input (s).
    pub fn delay(&self) -> f64 {
        (self.taps.len() - 1) as f64 / 2.0 / self.rate
    }

    /// The delay of a 12 kS/s audio channel (s).
    pub fn audio_delay() -> f64 {
        (TAPS_12K - 1) as f64 / 2.0 / AUDIO_RATE
    }

    fn one(&mut self, v: Complex32, out: &mut Vec<Complex32>) {
        let n = self.taps.len();
        let m = v * Complex32::new(self.rot.re as f32, self.rot.im as f32);
        self.rot *= self.step;
        self.since_norm += 1;
        if self.since_norm == 1024 {
            self.since_norm = 0;
            self.rot /= self.rot.norm();
        }
        self.ring[self.pos] = m;
        self.ring[self.pos + n] = m;
        self.pos = (self.pos + 1) % n;
        self.count += 1;
        if self.count == self.decim {
            self.count = 0;
            // ring[pos..pos+n] is oldest..newest; the taps are symmetric
            let mut acc = Complex32::new(0.0, 0.0);
            for (x, h) in self.ring[self.pos..self.pos + n].iter().zip(&self.taps) {
                acc += x * *h;
            }
            out.push(acc);
        }
    }

    /// Feed real audio; appends channel samples to `out`.
    pub fn push(&mut self, audio: &[f32], out: &mut Vec<Complex32>) {
        for &a in audio {
            // ×2: a real tone has half its power at +f
            self.one(Complex32::new(2.0 * a, 0.0), out);
        }
    }

    /// Feed complex IQ; appends channel samples to `out`.
    pub fn push_iq(&mut self, iq: &[Complex32], out: &mut Vec<Complex32>) {
        for &v in iq {
            self.one(v, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tone at the channel's frequency comes out at 0 Hz with its level;
    /// one 300 Hz away is stopped.
    #[test]
    fn mixes_and_filters_iq() {
        let rate = 60_000.0;
        let tone = |f: f64, n: usize| -> Vec<Complex32> {
            (0..n)
                .map(|i| {
                    let a = 2.0 * std::f64::consts::PI * f * i as f64 / rate;
                    Complex32::new(a.cos() as f32, a.sin() as f32)
                })
                .collect()
        };
        for (f, want) in [(-12_345.0, 1.0), (-12_045.0, 0.0)] {
            let mut ch = Channel::with_rate(-12_345.0, rate);
            let mut out = Vec::new();
            ch.push_iq(&tone(f, 60_000), &mut out);
            assert_eq!(out.len(), 500);
            let lvl = out[100..].iter().map(|v| v.norm()).sum::<f32>() / 400.0;
            assert!((lvl - want).abs() < 0.02, "{f}: {lvl}");
            if want > 0.0 {
                // at 0 Hz: the phase stands still
                let d = (out[300] * out[299].conj()).arg();
                assert!(d.abs() < 1e-3, "{d}");
            }
        }
    }
}
