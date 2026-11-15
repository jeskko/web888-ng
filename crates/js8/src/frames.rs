// SPDX-License-Identifier: GPL-3.0-or-later
//! Frames from a period of audio. Every submode is Normal's frame at
//! another symbol length, so its period is resampled in the frequency
//! domain to Normal's timing (Fast ×1.6, Turbo ×3.2 in time; Slow ×0.5,
//! its band in two halves since that doubles the frequencies) and
//! decoded by the FT8 decoder set up for JS8 (`web888_ftx::js8`).

use crate::submode::Submode;
use num_complex::Complex32;
use rustfft::FftPlanner;
use web888_ftx::ft8::{self, Config};
use web888_ftx::msg::CallHashes;

pub const RATE: f32 = 12_000.0;
/// What the FT8 decoder takes: 15 s.
const SLOT: usize = 15 * 12_000;
/// The audio band searched (Hz).
pub const F_MIN: f32 = 100.0;
pub const F_MAX: f32 = 3000.0;

/// A decoded frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub submode: Submode,
    /// Audio frequency of tone 0 (Hz).
    pub freq: f32,
    /// Start against the nominal (s).
    pub dt: f32,
    /// SNR in 2500 Hz (dB).
    pub snr: f32,
    /// The 12 characters and the frame type (3 bits).
    pub text: String,
    pub kind: u8,
    /// The 75 bits.
    pub bits: u128,
}

pub struct Frames {
    dec: Vec<(Submode, ft8::Decoder)>,
    planner: FftPlanner<f32>,
}

impl Default for Frames {
    fn default() -> Self {
        Self::new()
    }
}

impl Frames {
    pub fn new() -> Self {
        Frames {
            dec: Vec::new(),
            planner: FftPlanner::new(),
        }
    }

    fn decoder(&mut self, sm: Submode, f_min: f32, f_max: f32) -> &mut ft8::Decoder {
        let i = match self.dec.iter().position(|(s, _)| *s == sm) {
            Some(i) => i,
            None => {
                let cfg = Config {
                    js8: Some(sm.costas()),
                    t0: sm.start() * sm.stretch() as f32,
                    osd: None,
                    ..Config::default()
                };
                self.dec.push((sm, ft8::Decoder::new(cfg)));
                self.dec.len() - 1
            }
        };
        let _ = (f_min, f_max);
        &mut self.dec[i].1
    }

    /// Decodes a period of `sm` (its audio from the period's start).
    pub fn decode(&mut self, sm: Submode, audio: &[f32]) -> Vec<Frame> {
        let factor = sm.stretch();
        // input bands: the whole one, or Slow's two halves
        let bands: &[f32] = if sm == Submode::Slow {
            &[0.0, 1400.0]
        } else {
            &[0.0]
        };
        let mut out: Vec<Frame> = Vec::new();
        for &lo in bands {
            let x = if sm == Submode::Normal {
                audio.to_vec()
            } else {
                self.stretch(audio, factor, lo)
            };
            let mut hashes = CallHashes::default();
            let dec = self.decoder(sm, F_MIN, F_MAX);
            for d in dec.decode(&x, &mut hashes) {
                let freq = lo + d.freq * factor as f32;
                if !(F_MIN..=F_MAX).contains(&freq) || out.iter().any(|f| f.bits == d.bits) {
                    continue;
                }
                out.push(Frame {
                    submode: sm,
                    freq,
                    dt: d.dt / factor as f32,
                    snr: d.snr + 10.0 * (sm.baud() / Submode::Normal.baud()).log10(),
                    text: d.message.text.clone(),
                    kind: d.message.i3,
                    bits: d.bits,
                });
            }
        }
        out.sort_by(|a, b| a.freq.total_cmp(&b.freq));
        out
    }

    /// `audio` stretched in time by `factor` (frequencies divided by it),
    /// the input band from `lo` Hz up mapped to 0 Hz; 15 s of it.
    pub fn stretch(&mut self, audio: &[f32], factor: f64, lo: f32) -> Vec<f32> {
        let l = audio.len();
        let lo_out = (l as f64 * factor).round() as usize;
        let fft = self.planner.plan_fft_forward(l);
        let mut x: Vec<Complex32> = audio.iter().map(|&v| Complex32::new(v, 0.0)).collect();
        fft.process(&mut x);
        // output bin k is input bin k + j0 (both 12000 / l... Hz apart in
        // their own time bases); the positive half only, then twice the
        // real part
        let j0 = (lo as f64 * l as f64 / RATE as f64).round() as usize;
        let mut y = vec![Complex32::new(0.0, 0.0); lo_out];
        let n = (lo_out / 2).min((l / 2).saturating_sub(j0));
        if n > 1 {
            y[1..n].copy_from_slice(&x[1 + j0..n + j0]);
        }
        let ifft = self.planner.plan_fft_inverse(lo_out);
        ifft.process(&mut y);
        let scale = 2.0 / l as f32;
        let mut out: Vec<f32> = y.iter().take(SLOT).map(|v| v.re * scale).collect();
        out.resize(SLOT, 0.0);
        out
    }
}
