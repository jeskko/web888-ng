// SPDX-License-Identifier: GPL-3.0-or-later
//! Every signal in a wide window: 4.096 s of complex baseband at any rate
//! (a channelizer bin, 60 kS/s and ~40 kHz clean), its signals found
//! as the passband scanner finds them (`scan::regions_in`, the same
//! ~23 Hz bins) and any of them identified, one network run each, from
//! one FFT of the whole window. The page's "label every signal" hops
//! such windows across the waterfall's view.

use crate::measure::iq_len;
use crate::scan::{extract, regions_in, welch};
use num_complex::Complex32;
use rustfft::FftPlanner;

/// The spectrum's bin width (Hz), as the audio scanner's.
pub const BIN_HZ: f64 = 12_000.0 / 512.0;

pub struct Window {
    rate: f64,
    /// The window's spectrum, FFT order.
    spec: Vec<Complex32>,
    /// Power per BIN_HZ, lowest frequency (-rate/2) first.
    psd: Vec<f64>,
}

impl Window {
    /// The last 4.096 s of `iq` at `rate`; None if there is less.
    pub fn new(iq: &[Complex32], rate: f64) -> Option<Window> {
        let n = iq_len(rate);
        if iq.len() < n || rate <= 0.0 {
            return None;
        }
        let x = &iq[iq.len() - n..];
        let nf = ((rate / BIN_HZ).round() as usize).max(16);
        let mut psd = welch(x, nf);
        psd.rotate_left(nf / 2);
        let mut spec = x.to_vec();
        FftPlanner::new().plan_fft_forward(n).process(&mut spec);
        Some(Window { rate, spec, psd })
    }

    pub fn rate(&self) -> f64 {
        self.rate
    }

    /// The signals between `lo` and `hi` Hz from the centre: (lo, hi,
    /// peak dB over the floor), strongest first, at most `max`.
    pub fn regions(&self, lo: f64, hi: f64, max: usize) -> Vec<(f64, f64, f64)> {
        let nf = self.psd.len();
        let df = self.rate / nf as f64;
        regions_in(&self.psd, -((nf / 2) as f64) * df, df, lo, hi, max)
    }

    /// The identifier's input for a signal at `hz` from the centre.
    pub fn baseband(&self, hz: f64) -> Vec<Complex32> {
        extract(&self.spec, self.rate, hz)
    }

    /// The classes of the signal at `hz`, most likely first.
    pub fn identify(&self, hz: f64) -> Vec<(&'static str, f32)> {
        crate::identify(&self.baseband(hz))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two keyed carriers either side of the centre, in noise: both found
    /// where they are, and the identifier sees a carrier at 0 Hz.
    #[test]
    fn finds_both_sides() {
        let rate = 60_000.0;
        let n = iq_len(rate);
        let mut rng = 7u64;
        let mut noise = || {
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (rng >> 33) as f32 / (1u64 << 31) as f32 - 0.5
        };
        let tone = |f: f64, i: usize| {
            let a = 2.0 * std::f64::consts::PI * f * i as f64 / rate;
            Complex32::new(a.cos() as f32, a.sin() as f32)
        };
        let x: Vec<Complex32> = (0..n)
            .map(|i| {
                0.2 * tone(-12_345.0, i)
                    + 0.2 * tone(8_000.0, i)
                    + 0.01 * Complex32::new(noise(), noise())
            })
            .collect();
        let w = Window::new(&x, rate).expect("a window");
        let r = w.regions(-20_000.0, 20_000.0, 48);
        assert_eq!(r.len(), 2, "{r:?}");
        let mut c: Vec<f64> = r.iter().map(|r| 0.5 * (r.0 + r.1)).collect();
        c.sort_by(|a, b| a.total_cmp(b));
        assert!(
            (c[0] + 12_345.0).abs() < 40.0 && (c[1] - 8_000.0).abs() < 40.0,
            "{c:?}"
        );
        // the window's input: the carrier at 0 Hz, steady
        let y = w.baseband(8_000.0);
        let d = (y[300] * y[299].conj()).arg();
        assert!(d.abs() < 0.01, "phase step {d}");
        assert!((y[300].norm() - 0.2).abs() < 0.02, "{}", y[300].norm());
        assert!(Window::new(&x[..n - 1], rate).is_none());
    }
}
