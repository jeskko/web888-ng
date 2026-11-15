// SPDX-License-Identifier: GPL-3.0-or-later
//! One BPSK signal, mixed to near 0 Hz and at [`SPS`] samples a symbol.
//!
//! * Matched filter: a Hann window two symbols long (PSK31's pulse).
//! * Symbol timing: the filter output's energy, averaged per sampling
//!   phase over ~32 symbols; the strongest phase is sampled (reversals
//!   dip to zero between symbols, so the centres are the maxima).
//! * Differential detection: p = s_k · conj(s_{k−1}); no phase change
//!   (a 1) has p > 0, a reversal (a 0) p < 0. c = Re p / |p| is the
//!   cosine of the phase change: ±1 clean, 0 at 90°.
//! * Frequency loop: the phase change doubled (p², which removes the
//!   modulation) is the residual offset; a slow loop follows it.
//! * Squelch: the average |c| (noise alone gives 2/π ≈ 0.64).
//! * Is it BPSK? Per block of 32 symbols: p² keeps one phase (changes of
//!   0 or 180° only), the changes are clean, and some are reversals (a
//!   keyed carrier, CW, has none): see [`Demod::bpsk_blocks`].
//! * A character's margin is the smallest |c| of its bits (delimiter
//!   included): how near one came to reading otherwise.

use crate::varicode::Decoder;
use num_complex::Complex32;

/// Samples a symbol.
pub const SPS: usize = 16;

/// A decoded character: (stream time s, character, margin).
pub type Item = (f64, char, f32);

pub struct Demod {
    pub baud: f64,
    fs: f64,
    mf: [f32; 2 * SPS],
    ring: [Complex32; 4 * SPS],
    pos: usize,
    n: u64,
    t0: f64,
    energy: [f32; SPS],
    phase: usize,
    prev: Option<Complex32>,
    /// Frequency correction (Hz) and the mixer's phase.
    pub freq: f64,
    ph: f64,
    /// Average |c|.
    pub quality: f32,
    pub open: bool,
    vari: Decoder,
    margin: f32,
    /// Stream time of the last character given while open.
    pub last_text: f64,
    symbols: u64,
    blk: Block,
    good: u32,
}

/// One block's sums: p²/|p| and |p| (p the phase change), |c|, clean
/// reversals, noisy changes, symbols.
#[derive(Default)]
struct Block {
    s2: num_complex::Complex64,
    pw: f64,
    abs_c: f64,
    clean_rev: u32,
    noisy: u32,
    n: u32,
}

impl Block {
    /// Clean BPSK: p² coherent, the changes clean (mean |c| over 0.85, at
    /// most 10 % with |c| under 0.5), at least 20 % of them clean
    /// reversals (c under −0.85).
    fn bpsk(&self) -> bool {
        let n = self.n.max(1) as f64;
        self.pw > 0.0
            && self.s2.norm() / self.pw > 0.6
            && self.abs_c / n > 0.85
            && f64::from(self.clean_rev) / n >= 0.2
            && f64::from(self.noisy) / n <= 0.1
    }
}

impl Demod {
    /// Opens at this average |c|, closes below CLOSE.
    pub const OPEN: f32 = 0.8;
    pub const CLOSE: f32 = 0.72;

    /// `t0`: stream time of the first sample given.
    pub fn new(baud: f64, t0: f64) -> Self {
        let mut mf = [0f32; 2 * SPS];
        let s: f32 = (0..2 * SPS)
            .map(|i| {
                let v = (0.5
                    - 0.5
                        * (2.0 * std::f64::consts::PI * (i as f64 + 0.5) / (2 * SPS) as f64).cos())
                    as f32;
                mf[i] = v;
                v
            })
            .sum();
        for v in &mut mf {
            *v /= s;
        }
        Demod {
            baud,
            fs: baud * SPS as f64,
            mf,
            ring: [Complex32::new(0.0, 0.0); 4 * SPS],
            pos: 0,
            n: 0,
            t0,
            energy: [0.0; SPS],
            phase: 0,
            prev: None,
            freq: 0.0,
            ph: 0.0,
            quality: 0.64,
            open: false,
            vari: Decoder::default(),
            margin: f32::INFINITY,
            last_text: f64::NEG_INFINITY,
            symbols: 0,
            blk: Block::default(),
            good: 0,
        }
    }

    /// Blocks of 32 symbols that read as clean BPSK so far. The phase
    /// change p is 0 or 180° (plus the offset's turn), so p² keeps one
    /// phase: |Σ p²/|p|| / Σ |p| over 0.6 (~1 BPSK, ~0.18 noise, low for
    /// FSK, whose phase changes take other values); the changes are clean
    /// (mean |c| over 0.85, few under 0.5); and at least 20 % are clean
    /// reversals (text has one for each 0 bit, a third or more; a carrier
    /// or CW none, FSK and noise few). Measured on the 2026-11-15 40 m
    /// recording: real PSK31 blocks |c| ~1 with 30-50 % clean reversals,
    /// RTTY and CW taken for BPSK 0.7-0.9 with at most 16 %.
    pub fn bpsk_blocks(&self) -> u32 {
        self.good
    }

    pub fn now(&self) -> f64 {
        self.t0 + self.n as f64 / self.fs
    }

    pub fn feed(&mut self, x: &[Complex32]) -> Vec<Item> {
        let mut out = Vec::new();
        let m = 2 * SPS;
        let two_pi = 2.0 * std::f64::consts::PI;
        for &v in x {
            let z = v * Complex32::new(self.ph.cos() as f32, -self.ph.sin() as f32);
            self.ph = (self.ph + two_pi * self.freq / self.fs) % two_pi;
            // ring holds the last m samples twice over for a contiguous view
            let l = self.ring.len() / 2;
            self.ring[self.pos] = z;
            self.ring[self.pos + l] = z;
            self.pos = (self.pos + 1) % l;
            let w = &self.ring[self.pos + l - m..self.pos + l];
            let y: Complex32 = w.iter().zip(&self.mf).map(|(a, &h)| a * h).sum();
            let k = (self.n % SPS as u64) as usize;
            self.n += 1;
            self.energy[k] += (y.norm_sqr() - self.energy[k]) / 32.0;
            if k == 0 {
                let best = (0..SPS)
                    .max_by(|&a, &b| self.energy[a].total_cmp(&self.energy[b]))
                    .unwrap_or(0);
                if self.energy[best] > 1.1 * self.energy[self.phase] {
                    self.phase = best;
                }
            }
            if k != self.phase {
                continue;
            }
            self.symbols += 1;
            let Some(p0) = self.prev.replace(y) else {
                continue;
            };
            let p = y * p0.conj();
            let mag = p.norm();
            if mag <= 0.0 {
                continue;
            }
            let c = p.re / mag;
            // the residual offset: half the angle of p² (rad a symbol)
            let e = (p * p).arg() as f64 / 2.0;
            let gain = if self.symbols < 64 { 0.05 } else { 0.01 };
            self.freq = (self.freq + gain * e * self.baud / two_pi)
                .clamp(-0.3 * self.baud, 0.3 * self.baud);
            self.quality += (c.abs() - self.quality) / 20.0;
            if self.symbols > 32 {
                // (after the timing has settled)
                // p² (the phase change doubled: no modulation left, and an
                // offset only turns it), normalised
                let q = p * p / mag;
                let b = &mut self.blk;
                b.s2 += num_complex::Complex64::new(q.re as f64, q.im as f64);
                b.pw += mag as f64;
                b.abs_c += c.abs() as f64;
                b.clean_rev += u32::from(c < -0.85);
                b.noisy += u32::from(c.abs() < 0.5);
                b.n += 1;
                if b.n == 32 {
                    self.good += u32::from(b.bpsk());
                    self.blk = Block::default();
                }
            }
            if self.quality > Self::OPEN {
                self.open = true;
            } else if self.quality < Self::CLOSE {
                self.open = false;
            }
            self.margin = self.margin.min(c.abs());
            // the symbol's centre was a symbol and the filter's half ago
            let t = self.now() - 1.5 / self.baud;
            if let Some(ch) = self.vari.push(c > 0.0) {
                if self.open {
                    out.push((t, ch, self.margin));
                    self.last_text = t;
                }
                self.margin = f32::INFINITY;
            }
        }
        out
    }
}
