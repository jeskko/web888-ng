// SPDX-License-Identifier: GPL-3.0-or-later
//! One beacon, mixed to near 0 Hz, at [`SPS`] samples a bit.
//!
//! * MSK turns the carrier's phase by +90° over a bit for a 1 and −90°
//!   for a 0: p = s_n · conj(s_{n−SPS}) is that turn when n ends a bit,
//!   and the bit is its sign.
//! * Bit timing: |arg p| per sampling phase, averaged over ~32 bits; the
//!   phase where it is largest ends the bits (elsewhere the window spans
//!   parts of two bits, which cancel half the time).
//! * Frequency: an offset Δf adds 2π·Δf·T to every turn; doubled, the
//!   turns are all −1 (±180°) but for that, so a slow loop follows
//!   arg(−p²)/2.
//! * Quality: the average |arg p| over 90° (1 clean; noise ~0.5).
//!
//! The bits go to an RTCM [`Framer`]; the stream's polarity doesn't
//! matter (RTCM's parity reads an inverted stream the same).

use crate::rtcm::{Framer, Msg};
use num_complex::{Complex32, Complex64};
use std::f64::consts::PI;

/// Samples a bit (12: the channel's rate divides 12 kS/s for 50, 100
/// and 200 Bd).
pub const SPS: usize = 12;

pub struct Demod {
    pub baud: f64,
    fs: f64,
    ring: [Complex32; SPS],
    n: u64,
    t0: f64,
    energy: [f32; SPS],
    phase: usize,
    /// Frequency correction (Hz) and the mixer's phase.
    pub freq: f64,
    ph: f64,
    turn2: Complex64,
    /// Average |turn| over 90°.
    pub quality: f32,
    pub framer: Framer,
}

impl Demod {
    /// `t0`: stream time of the first sample fed.
    pub fn new(baud: f64, t0: f64) -> Self {
        Demod {
            baud,
            fs: baud * SPS as f64,
            ring: [Complex32::new(0.0, 0.0); SPS],
            n: 0,
            t0,
            energy: [0.0; SPS],
            phase: 0,
            freq: 0.0,
            ph: 0.0,
            turn2: Complex64::new(0.0, 0.0),
            quality: 0.5,
            framer: Framer::new(1.0 / baud),
        }
    }

    /// Feed channel samples; the messages that ended in them.
    pub fn feed(&mut self, x: &[Complex32]) -> Vec<Msg> {
        let mut out = Vec::new();
        let step = 2.0 * PI / self.fs;
        for &v in x {
            let r = Complex64::from_polar(1.0, -self.ph);
            self.ph = (self.ph + step * self.freq) % (2.0 * PI);
            let v = v * Complex32::new(r.re as f32, r.im as f32);
            let k = (self.n % SPS as u64) as usize;
            let prev = self.ring[k];
            self.ring[k] = v;
            self.n += 1;
            if self.n <= SPS as u64 {
                continue;
            }
            let p = v * prev.conj();
            let a = p.arg();
            self.energy[k] += (a.abs() - self.energy[k]) / 32.0;
            if k == self.phase {
                let t = self.t0 + self.n as f64 / self.fs;
                self.quality += (a.abs() / (PI as f32 / 2.0) - self.quality) / 64.0;
                if p.norm_sqr() > 0.0 {
                    // the doubled turn, less the modulation: -p²
                    let q = Complex64::from_polar(1.0, 2.0 * f64::from(a));
                    self.turn2 = self.turn2 * 0.97 - q * 0.03;
                    if self.turn2.norm() > 0.3 {
                        let eps = self.turn2.arg() / 2.0;
                        self.freq += 0.02 * eps / (2.0 * PI) * self.baud;
                    }
                }
                if let Some(m) = self.framer.push(a > 0.0, t) {
                    out.push(m);
                }
            }
            if k == SPS - 1 {
                let best = (0..SPS)
                    .max_by(|&i, &j| self.energy[i].total_cmp(&self.energy[j]))
                    .unwrap_or(0);
                if self.energy[best] > 1.05 * self.energy[self.phase] {
                    self.phase = best;
                }
            }
        }
        out
    }
}
