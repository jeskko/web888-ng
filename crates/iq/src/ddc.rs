// SPDX-License-Identifier: GPL-3.0-or-later
//! Digital down-conversion of a complex wideband input, structured like the
//! FPGA chains but at the input's lower rate:
//!
//! * RX: NCO mix, CIC N=3 down to 480 kS/s (the FPGA's CIC1 output rate),
//!   CIC N=5 R=r2 (identical to the FPGA's CIC2: same input rate);
//! * WF: NCO mix, CIC N=5 down to the WF rate.
//!
//! CICs run on wrapping i64 integers, which is exact as long as the true
//! output fits; each stage scales its input for that and the output is
//! normalised to unit DC gain.

use num_complex::{Complex32, Complex64};

/// Input scale for an N-stage CIC with decimation R: leaves room for the
/// R^N gain and a factor 2 of input headroom in an i64.
fn input_scale(n: u32, r: u32) -> f64 {
    let growth = n as f64 * (r as f64).log2();
    2f64.powf((61.0 - growth).floor().min(24.0))
}

/// Complex N-stage CIC decimator (differential delay 1), unit DC gain.
#[derive(Clone, Debug)]
pub struct Cic {
    n: usize,
    r: u32,
    integ: [(i64, i64); 5],
    comb: [(i64, i64); 5],
    phase: u32,
    scale: f64,
    /// 1 / (scale * R^N)
    out_gain: f64,
}

impl Cic {
    pub fn new(n: usize, r: u32) -> Self {
        assert!((1..=5).contains(&n) && r >= 1);
        let scale = input_scale(n as u32, r);
        Cic {
            n,
            r,
            integ: [(0, 0); 5],
            comb: [(0, 0); 5],
            phase: 0,
            scale,
            out_gain: 1.0 / (scale * (r as f64).powi(n as i32)),
        }
    }

    pub fn decimation(&self) -> u32 {
        self.r
    }

    /// Set the decimation phase: as if `inputs` samples had been pushed.
    pub fn set_phase(&mut self, inputs: u64) {
        self.phase = (inputs % self.r as u64) as u32;
    }

    /// One input sample; every R-th call returns an output.
    #[inline]
    pub fn push(&mut self, x: Complex64) -> Option<Complex64> {
        // truncation, not round(): one instruction instead of a libm call
        // per sample; the half-LSB bias at >= 2^15 scale is negligible
        let (mut a, mut b) = ((x.re * self.scale) as i64, (x.im * self.scale) as i64);
        for s in &mut self.integ[..self.n] {
            s.0 = s.0.wrapping_add(a);
            s.1 = s.1.wrapping_add(b);
            (a, b) = *s;
        }
        self.phase += 1;
        if self.phase < self.r {
            return None;
        }
        self.phase = 0;
        for c in &mut self.comb[..self.n] {
            let (pa, pb) = *c;
            *c = (a, b);
            a = a.wrapping_sub(pa);
            b = b.wrapping_sub(pb);
        }
        Some(Complex64::new(a as f64, b as f64) * self.out_gain)
    }
}

/// NCO: `x * exp(-j w n)` (a signal at `f_nco + d` appears at `+d`).
#[derive(Clone, Debug)]
pub struct Nco {
    ph: Complex64,
    rot: Complex64,
    count: u32,
}

impl Nco {
    /// `offset_hz` = NCO frequency minus the input's centre frequency.
    pub fn new(offset_hz: f64, fs_in: f64) -> Self {
        Nco {
            ph: Complex64::new(1.0, 0.0),
            rot: Complex64::from_polar(1.0, -2.0 * std::f64::consts::PI * offset_hz / fs_in),
            count: 0,
        }
    }

    /// Retune, phase-continuously.
    pub fn set_offset(&mut self, offset_hz: f64, fs_in: f64) {
        self.rot = Complex64::from_polar(1.0, -2.0 * std::f64::consts::PI * offset_hz / fs_in);
    }

    #[inline]
    pub fn mix(&mut self, x: Complex32) -> Complex64 {
        let y = Complex64::new(x.re as f64, x.im as f64) * self.ph;
        self.ph *= self.rot;
        self.count += 1;
        if self.count == 4096 {
            self.count = 0;
            self.ph /= self.ph.norm();
        }
        y
    }
}

/// One channel: NCO and a cascade of CICs.
#[derive(Clone, Debug)]
pub struct Ddc {
    nco: Nco,
    stages: Vec<Cic>,
}

impl Ddc {
    /// RX: `r1` = input rate / 480 kS/s, then R = `r2`.
    pub fn rx(offset_hz: f64, fs_in: f64, r1: u32, r2: u32) -> Self {
        Ddc {
            nco: Nco::new(offset_hz, fs_in),
            stages: vec![Cic::new(3, r1), Cic::new(5, r2)],
        }
    }

    /// WF: one CIC N=5 with decimation `r`.
    pub fn wf(offset_hz: f64, fs_in: f64, r: u32) -> Self {
        Ddc {
            nco: Nco::new(offset_hz, fs_in),
            stages: vec![Cic::new(5, r)],
        }
    }

    /// Retune, keeping the filter state and decimation phase.
    pub fn set_offset(&mut self, offset_hz: f64, fs_in: f64) {
        self.nco.set_offset(offset_hz, fs_in);
    }

    /// Start in step with chains that have already taken `k` input
    /// samples, so all produce their outputs on the same input sample.
    pub fn align(&mut self, k: u64) {
        let mut n = k;
        for s in &mut self.stages {
            s.set_phase(n);
            n /= s.decimation() as u64;
        }
    }

    /// Total decimation.
    pub fn decimation(&self) -> u32 {
        self.stages.iter().map(Cic::decimation).product()
    }

    /// A block of input; `out` gets the completed outputs (input units).
    pub fn process(&mut self, x: &[Complex32], out: &mut Vec<Complex64>) {
        out.clear();
        for &s in x {
            if let Some(y) = self.push(s) {
                out.push(y);
            }
        }
    }

    /// One input sample; returns an output sample (input units) when one
    /// is complete.
    #[inline]
    pub fn push(&mut self, x: Complex32) -> Option<Complex64> {
        let mut y = self.nco.mix(x);
        for s in &mut self.stages {
            y = s.push(y)?;
        }
        Some(y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(f: f64, fs: f64, n: usize, amp: f32) -> Vec<Complex32> {
        (0..n)
            .map(|k| {
                let p = 2.0 * std::f64::consts::PI * f * k as f64 / fs;
                Complex32::new(amp * p.cos() as f32, amp * p.sin() as f32)
            })
            .collect()
    }

    #[test]
    fn cic_dc_gain_is_one_and_never_overflows() {
        for (n, r) in [(3, 32), (5, 40), (5, 1024), (5, 1)] {
            let mut c = Cic::new(n, r);
            let mut last = None;
            for _ in 0..(r as usize * 20) {
                if let Some(y) = c.push(Complex64::new(1.0, -1.0)) {
                    last = Some(y);
                }
            }
            let y = last.unwrap();
            assert!(
                (y.re - 1.0).abs() < 1e-6 && (y.im + 1.0).abs() < 1e-6,
                "{n} {r}: {y}"
            );
        }
    }

    #[test]
    fn rx_channel_shifts_and_keeps_level() {
        // 7.68 MS/s input, tone 1 kHz above the NCO, RX at 48 kS/s
        let fs = 7.68e6;
        let x = tone(1_001_000.0, fs, 160 * 4000, 0.5);
        let mut d = Ddc::rx(1_000_000.0, fs, 16, 10);
        assert_eq!(d.decimation(), 160);
        let out: Vec<Complex64> = x.iter().filter_map(|&s| d.push(s)).collect();
        assert_eq!(out.len(), 4000);
        let tail = &out[1000..];
        let amp = tail.iter().map(|z| z.norm()).sum::<f64>() / tail.len() as f64;
        // CIC droop at 1 kHz of a 48 kS/s output is ~0.01 dB
        assert!((amp - 0.5).abs() < 0.005, "{amp}");
        // phase advances by +1 kHz / 48 kS/s per sample
        let w = (tail[1] * tail[0].conj()).arg();
        let want = 2.0 * std::f64::consts::PI * 1000.0 / 48_000.0;
        assert!((w - want).abs() < 1e-3, "{w} {want}");
    }
}
