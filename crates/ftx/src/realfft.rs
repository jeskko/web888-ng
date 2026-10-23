// SPDX-License-Identifier: GPL-3.0-or-later
//! Real-input FFT of even length N through a complex FFT of length N/2,
//! and its inverse: half the work of a complex FFT of the zero-imaginary
//! signal.

use num_complex::Complex32 as C32;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

pub struct RealFft {
    n: usize,
    fwd: Arc<dyn Fft<f32>>,
    inv: Arc<dyn Fft<f32>>,
    /// exp(-j 2 pi k / N), k < N/2.
    tw: Vec<C32>,
}

impl RealFft {
    pub fn new(n: usize) -> Self {
        assert!(n.is_multiple_of(2));
        let mut p = FftPlanner::new();
        let tw = (0..n / 2)
            .map(|k| {
                let a = -2.0 * std::f64::consts::PI * k as f64 / n as f64;
                C32::new(a.cos() as f32, a.sin() as f32)
            })
            .collect();
        RealFft {
            n,
            fwd: p.plan_fft_forward(n / 2),
            inv: p.plan_fft_inverse(n / 2),
            tw,
        }
    }

    /// Bins 0..=N/2 of the (unnormalised) DFT of `x`, zero-padded to N.
    pub fn forward(&self, x: &[f32]) -> Vec<C32> {
        let h = self.n / 2;
        let at = |i: usize| x.get(i).copied().unwrap_or(0.0);
        let mut z: Vec<C32> = (0..h).map(|i| C32::new(at(2 * i), at(2 * i + 1))).collect();
        self.fwd.process(&mut z);
        let mut out: Vec<C32> = (0..h)
            .map(|k| {
                let a = z[k];
                let b = z[(h - k) % h].conj();
                let even = (a + b) * 0.5;
                let odd = (a - b) * C32::new(0.0, -0.5);
                even + self.tw[k] * odd
            })
            .collect();
        // Nyquist: the even part minus the odd part at bin 0.
        out.push(C32::new(z[0].re - z[0].im, 0.0));
        out
    }

    /// The real signal (length N, scaled by N like an unnormalised
    /// inverse) whose bins 0..=N/2 are `spectrum` (missing ones are 0).
    pub fn inverse(&self, spectrum: &[C32]) -> Vec<f32> {
        let h = self.n / 2;
        let at = |k: usize| spectrum.get(k).copied().unwrap_or_default();
        let mut z: Vec<C32> = (0..h)
            .map(|k| {
                let a = at(k);
                let b = at(h - k).conj();
                let even = a + b;
                let odd = (a - b) * self.tw[k].conj();
                even + odd * C32::new(0.0, 1.0)
            })
            .collect();
        self.inv.process(&mut z);
        let mut x = Vec::with_capacity(self.n);
        for v in z {
            x.push(v.re);
            x.push(v.im);
        }
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_complex_fft() {
        let n = 1200;
        let x: Vec<f32> = (0..1000)
            .map(|i| ((i * 7919) % 1013) as f32 / 1013.0 - 0.5)
            .collect();
        let r = RealFft::new(n);
        let got = r.forward(&x);
        let mut full: Vec<C32> = (0..n)
            .map(|i| C32::new(x.get(i).copied().unwrap_or(0.0), 0.0))
            .collect();
        FftPlanner::new().plan_fft_forward(n).process(&mut full);
        for k in 0..=n / 2 {
            assert!(
                (got[k] - full[k]).norm() < 1e-3,
                "bin {k}: {} vs {}",
                got[k],
                full[k]
            );
        }
        let back = r.inverse(&got);
        for (i, b) in back.iter().enumerate() {
            let want = x.get(i).copied().unwrap_or(0.0);
            assert!((b / n as f32 - want).abs() < 1e-4, "sample {i}");
        }
    }
}
