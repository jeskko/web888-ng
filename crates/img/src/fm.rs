// SPDX-License-Identifier: GPL-3.0-or-later
//! FM demodulation of the image modes' audio: real 12 kHz audio mixed
//! from 1900 Hz to complex, low-passed (±1200 Hz: black 1500, white 2300,
//! sync 1200, VIS 1100/1300 and their sidebands) and decimated to
//! [`FM_RATE`]; the instantaneous frequency in Hz of audio.

use num_complex::Complex32;
use web888_cw::dsp;

pub const AUDIO_RATE: f64 = 12_000.0;
const DECIM: usize = 2;
/// The output rate.
pub const FM_RATE: f64 = AUDIO_RATE / DECIM as f64;
/// The mixing frequency.
pub const CENTRE: f64 = 1900.0;
const TAPS: usize = 41;

pub struct FmDemod {
    taps: Vec<f32>,
    ring: Vec<Complex32>,
    pos: usize,
    phase: f64,
    count: usize,
    prev: Complex32,
}

impl Default for FmDemod {
    fn default() -> Self {
        Self::new()
    }
}

impl FmDemod {
    pub fn new() -> Self {
        FmDemod {
            taps: dsp::firwin(TAPS, 1200.0, AUDIO_RATE),
            ring: vec![Complex32::new(0.0, 0.0); 2 * TAPS],
            pos: 0,
            phase: 0.0,
            count: 0,
            prev: Complex32::new(1.0, 0.0),
        }
    }

    /// The delay of the output behind the input (s).
    pub fn delay() -> f64 {
        (TAPS - 1) as f64 / 2.0 / AUDIO_RATE
    }

    /// Feeds audio; appends instantaneous frequencies (Hz of audio) and
    /// amplitudes.
    pub fn push(&mut self, audio: &[f32], freq: &mut Vec<f32>, amp: &mut Vec<f32>) {
        let step = -2.0 * std::f64::consts::PI * CENTRE / AUDIO_RATE;
        for &a in audio {
            let v = Complex32::new(
                (a as f64 * self.phase.cos()) as f32,
                (a as f64 * self.phase.sin()) as f32,
            );
            self.phase += step;
            if self.phase < -std::f64::consts::PI {
                self.phase += 2.0 * std::f64::consts::PI;
            }
            self.ring[self.pos] = v;
            self.ring[self.pos + TAPS] = v;
            self.pos = (self.pos + 1) % TAPS;
            self.count += 1;
            if self.count == DECIM {
                self.count = 0;
                let mut z = Complex32::new(0.0, 0.0);
                for (x, h) in self.ring[self.pos..self.pos + TAPS].iter().zip(&self.taps) {
                    z += x * *h;
                }
                let d = z * self.prev.conj();
                self.prev = z;
                freq.push(
                    (CENTRE + d.arg() as f64 * FM_RATE / (2.0 * std::f64::consts::PI)) as f32,
                );
                amp.push(2.0 * z.norm());
            }
        }
    }
}

/// Synthesis for tests: phase-continuous audio following `freq` (Hz) at
/// [`AUDIO_RATE`] (one value per sample).
pub fn tone_audio(freq: &[f32], amp: f32) -> Vec<f32> {
    let mut ph = 0.0f64;
    freq.iter()
        .map(|&f| {
            ph += 2.0 * std::f64::consts::PI * f as f64 / AUDIO_RATE;
            if ph > std::f64::consts::PI {
                ph -= 2.0 * std::f64::consts::PI;
            }
            amp * ph.cos() as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steady_tones() {
        for f in [1100.0f32, 1200.0, 1500.0, 1900.0, 2300.0] {
            let a = tone_audio(&vec![f; 6000], 0.3);
            let mut d = FmDemod::new();
            let (mut fr, mut am) = (Vec::new(), Vec::new());
            d.push(&a, &mut fr, &mut am);
            let mid = &fr[500..];
            let mean = mid.iter().sum::<f32>() / mid.len() as f32;
            assert!((mean - f).abs() < 2.0, "{f}: {mean}");
            assert!((am[1000] - 0.3).abs() < 0.02, "{}", am[1000]);
        }
    }
}
