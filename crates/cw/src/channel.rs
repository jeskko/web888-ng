// SPDX-License-Identifier: GPL-3.0-or-later
//! One signal's channel: real audio at [`AUDIO_RATE`] mixed down from the
//! signal's audio frequency and decimated to complex [`ENV_FS`]
//! (±210 Hz pass, the image at −2f and other signals stopped).

use crate::decode::ENV_FS;
use crate::dsp;
use num_complex::Complex32;

/// The input audio rate.
pub const AUDIO_RATE: f64 = 12_000.0;
const DECIM: usize = (AUDIO_RATE / ENV_FS) as usize;
const TAPS: usize = 289;

pub struct Channel {
    /// Audio frequency mixed to 0 (Hz).
    pub hz: f64,
    taps: Vec<f32>,
    /// The last TAPS mixed samples (a ring, doubled for contiguous reads).
    ring: Vec<Complex32>,
    pos: usize,
    phase: f64,
    step: f64,
    count: usize,
}

impl Channel {
    pub fn new(hz: f64) -> Self {
        Channel {
            hz,
            taps: dsp::firwin(TAPS, 210.0, AUDIO_RATE),
            ring: vec![Complex32::new(0.0, 0.0); 2 * TAPS],
            pos: 0,
            phase: 0.0,
            step: -2.0 * std::f64::consts::PI * hz / AUDIO_RATE,
            count: 0,
        }
    }

    /// The delay of the output behind the input (s).
    pub fn delay() -> f64 {
        (TAPS - 1) as f64 / 2.0 / AUDIO_RATE
    }

    /// Feed audio; appends channel samples to `out`.
    pub fn push(&mut self, audio: &[f32], out: &mut Vec<Complex32>) {
        for &a in audio {
            let v = Complex32::new(
                (a as f64 * self.phase.cos()) as f32,
                (a as f64 * self.phase.sin()) as f32,
            );
            self.phase += self.step;
            if self.phase < -std::f64::consts::PI {
                self.phase += 2.0 * std::f64::consts::PI;
            }
            self.ring[self.pos] = v;
            self.ring[self.pos + TAPS] = v;
            self.pos = (self.pos + 1) % TAPS;
            self.count += 1;
            if self.count == DECIM {
                self.count = 0;
                // ring[pos..pos+TAPS] is oldest..newest; taps are symmetric
                let w = &self.ring[self.pos..self.pos + TAPS];
                let mut acc = Complex32::new(0.0, 0.0);
                for (x, h) in w.iter().zip(&self.taps) {
                    acc += x * *h;
                }
                // ×2: the real input's tone has half its power at +f
                out.push(acc * 2.0);
            }
        }
    }
}
