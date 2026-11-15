// SPDX-License-Identifier: GPL-3.0-or-later
//! Input (audio, or complex IQ, at any rate) to complex baseband at
//! [`FS`] (4 samples a symbol), the burst's centre at 0 Hz, through a
//! root raised cosine matched filter.

use num_complex::Complex32;
use std::f64::consts::PI;

/// Output samples a second.
pub const FS: f64 = 7200.0;
/// Samples a symbol.
pub const SPS: usize = 4;
/// The matched filter's roll-off and half span (symbols).
const ROLLOFF: f64 = 0.3;
const MF_SPAN: usize = 6;
/// The resampler's pass and stop edges (Hz).
const PASS: f64 = 1250.0;
const STOP: f64 = 1700.0;
/// Resampler table phases.
const PHASES: usize = 64;

fn bessel_i0(x: f64) -> f64 {
    let (mut s, mut t) = (1.0, 1.0);
    for k in 1..40 {
        t *= (x / 2.0 / k as f64).powi(2);
        s += t;
    }
    s
}

/// Root raised cosine at t symbols.
pub fn rrc(t: f64, a: f64) -> f64 {
    if t.abs() < 1e-9 {
        return 1.0 - a + 4.0 * a / PI;
    }
    if (t.abs() - 1.0 / (4.0 * a)).abs() < 1e-9 {
        return a / 2f64.sqrt()
            * ((1.0 + 2.0 / PI) * (PI / (4.0 * a)).sin()
                + (1.0 - 2.0 / PI) * (PI / (4.0 * a)).cos());
    }
    ((PI * t * (1.0 - a)).sin() + 4.0 * a * t * (PI * t * (1.0 + a)).cos())
        / (PI * t * (1.0 - (4.0 * a * t).powi(2)))
}

/// The matched filter's taps (unit energy).
pub fn matched() -> Vec<f32> {
    let n = 2 * MF_SPAN * SPS + 1;
    let h: Vec<f64> = (0..n)
        .map(|i| rrc((i as f64 - (n / 2) as f64) / SPS as f64, ROLLOFF))
        .collect();
    let e = h.iter().map(|v| v * v).sum::<f64>().sqrt();
    h.iter().map(|v| (v / e) as f32).collect()
}

pub struct Front {
    /// Input samples an output sample.
    step: f64,
    taps: usize,
    /// PHASES + 1 rows of `taps`.
    table: Vec<f32>,
    hist: Vec<Complex32>,
    /// Input index of hist[0], and the next output's input time.
    hist0: u64,
    next: f64,
    mix: f64,
    dmix: f64,
    mf: Vec<f32>,
    mf_hist: Vec<Complex32>,
    /// Input samples taken.
    pub taken: u64,
    rate: f64,
}

impl Front {
    /// `rate`: input samples a second; `centre`: the burst's centre in the
    /// input (Hz).
    pub fn new(rate: f64, centre: f64) -> Self {
        // Kaiser (~60 dB): taps ≈ 3.6 rate / transition
        let taps = ((3.6 * rate / (STOP - PASS)).ceil() as usize).max(8) | 1;
        let fc = (PASS + STOP) / 2.0 / rate;
        let beta = 5.65;
        let half = (taps / 2) as f64;
        let mut table = vec![0f32; (PHASES + 1) * taps];
        for p in 0..=PHASES {
            let mu = p as f64 / PHASES as f64;
            for j in 0..taps {
                // tap j multiplies x[i + j - half]; h at (mu - (j - half))
                let t = mu - (j as f64 - half);
                let r = t / (half + 1.0);
                let w = if r.abs() < 1.0 {
                    bessel_i0(beta * (1.0 - r * r).sqrt()) / bessel_i0(beta)
                } else {
                    0.0
                };
                let s = if t.abs() < 1e-12 {
                    1.0
                } else {
                    (2.0 * PI * fc * t).sin() / (2.0 * PI * fc * t)
                };
                table[p * taps + j] = (2.0 * fc * s * w) as f32;
            }
        }
        let mf = matched();
        Front {
            step: rate / FS,
            taps,
            table,
            hist: Vec::new(),
            hist0: 0,
            next: half,
            mix: 0.0,
            dmix: -2.0 * PI * centre / rate,
            mf_hist: vec![Complex32::new(0.0, 0.0); mf.len()],
            mf,
            taken: 0,
            rate,
        }
    }

    pub fn rate(&self) -> f64 {
        self.rate
    }

    /// Output delay: an output sample's centre is this long (s) before the
    /// input time it's produced at.
    pub fn delay(&self) -> f64 {
        ((self.mf.len() / 2) as f64) / FS
    }

    /// Input time (s from the first sample) of output sample `m` (its
    /// centre).
    pub fn time(&self, m: u64) -> f64 {
        ((self.taps / 2) as f64 + m as f64 * self.step) / self.rate - self.delay()
    }

    pub fn push(&mut self, x: &[Complex32], out: &mut Vec<Complex32>) {
        for &v in x {
            let r = Complex32::new(self.mix.cos() as f32, self.mix.sin() as f32);
            self.mix = (self.mix + self.dmix) % (2.0 * PI);
            self.hist.push(v * r);
        }
        self.taken += x.len() as u64;
        let half = self.taps / 2;
        loop {
            let i = self.next.floor();
            let mu = self.next - i;
            let i = i as u64;
            if i + (self.taps - half) as u64 > self.hist0 + self.hist.len() as u64 {
                break;
            }
            let start = (i - half as u64 - self.hist0) as usize;
            let w = &self.hist[start..start + self.taps];
            let pf = mu * PHASES as f64;
            let p = (pf as usize).min(PHASES - 1);
            let fr = (pf - p as f64) as f32;
            let (a, b) = (
                &self.table[p * self.taps..(p + 1) * self.taps],
                &self.table[(p + 1) * self.taps..(p + 2) * self.taps],
            );
            let mut y = Complex32::new(0.0, 0.0);
            for j in 0..self.taps {
                y += w[j] * (a[j] + fr * (b[j] - a[j]));
            }
            self.next += self.step;
            // matched filter
            self.mf_hist.rotate_left(1);
            let l = self.mf_hist.len();
            self.mf_hist[l - 1] = y;
            let z: Complex32 = self.mf_hist.iter().zip(&self.mf).map(|(a, &h)| a * h).sum();
            out.push(z);
        }
        // drop history no longer needed
        let keep_from = (self.next.floor() as u64).saturating_sub(half as u64 + 1);
        if keep_from > self.hist0 + 4096 {
            let d = (keep_from - self.hist0) as usize;
            self.hist.drain(..d);
            self.hist0 += d as u64;
        }
    }
}
