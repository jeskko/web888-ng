// SPDX-License-Identifier: GPL-3.0-or-later
//! USB audio at 12 kS/s from a 48 kS/s channel tuned 3 kHz above the dial:
//! decimate by 4 keeping ±2.9 kHz, then shift up by 3 kHz (a quarter of
//! the output rate: multiplying by j^n) and take the real part. The audio
//! covers 0.1..5.9 kHz above the dial.

use web888_dsp::C32;
use web888_dsp::decim::Decimator;

pub const AUDIO_RATE: f64 = 12_000.0;
/// The channel is tuned this far above the dial.
pub const OFFSET_HZ: f64 = 3_000.0;

pub struct UsbAudio {
    dec: Decimator,
    n: usize,
    buf: Vec<C32>,
    out: Vec<C32>,
}

/// Re(z e^(j pi n / 2)) as z.re * c + z.im * s for n mod 4: products, not a
/// branch (a jump table that changes target every sample mispredicts on
/// the A9 every time).
const MIX: [(f32, f32); 4] = [(1.0, 0.0), (0.0, -1.0), (-1.0, 0.0), (0.0, 1.0)];

impl UsbAudio {
    pub fn new(rx_rate: f64) -> Self {
        let r = (rx_rate / AUDIO_RATE).round() as usize;
        assert!(
            (rx_rate - AUDIO_RATE * r as f64).abs() < 1.0,
            "rx rate must be a multiple of 12 kS/s"
        );
        UsbAudio {
            dec: Decimator::new(rx_rate, r, 2_900.0, 80.0),
            n: 0,
            buf: vec![],
            out: vec![],
        }
    }

    /// Append the audio of `iq` (channel samples) to `audio`.
    pub fn process(&mut self, iq: impl IntoIterator<Item = C32>, audio: &mut Vec<f32>) {
        if self.dec.passes_through() {
            // the channel is at the audio rate already: straight to the mix
            let mut n = self.n;
            audio.extend(iq.into_iter().map(|z| {
                let (c, s) = MIX[n & 3];
                n = n.wrapping_add(1);
                z.re * c + z.im * s
            }));
            self.n = n;
            return;
        }
        self.buf.clear();
        self.buf.extend(iq);
        self.out.clear();
        self.dec.process(&self.buf, &mut self.out);
        for i in 0..self.out.len() {
            let z = self.out[i];
            audio.push(self.mix(z));
        }
    }

    /// One sample shifted up a quarter of the rate, its real part.
    #[inline]
    fn mix(&mut self, z: C32) -> f32 {
        let (c, s) = MIX[self.n & 3];
        self.n = self.n.wrapping_add(1);
        z.re * c + z.im * s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A carrier 1 kHz above the dial comes out as a 1 kHz tone.
    #[test]
    fn carrier_becomes_audio_tone() {
        let fs = 48_000.0;
        let f = 1_000.0 - OFFSET_HZ; // relative to the channel centre
        let x: Vec<C32> = (0..48_000)
            .map(|n| {
                let p = 2.0 * std::f64::consts::PI * f * n as f64 / fs;
                C32::new(p.cos() as f32, p.sin() as f32)
            })
            .collect();
        let mut u = UsbAudio::new(fs);
        let mut a = vec![];
        u.process(x, &mut a);
        assert_eq!(a.len(), 12_000);
        // Correlate with 1 kHz: power there, little at 5 kHz (the mirror).
        let p = |hz: f64| {
            let (mut re, mut im) = (0.0, 0.0);
            for (n, &v) in a[2000..].iter().enumerate() {
                let ph = 2.0 * std::f64::consts::PI * hz * n as f64 / AUDIO_RATE;
                re += v as f64 * ph.cos();
                im += v as f64 * ph.sin();
            }
            (re * re + im * im).sqrt() / (a.len() - 2000) as f64
        };
        assert!((p(1000.0) - 0.5).abs() < 0.05, "{}", p(1000.0));
        assert!(p(5000.0) < 1e-3);
    }
}
