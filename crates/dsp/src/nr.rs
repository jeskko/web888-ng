// SPDX-License-Identifier: GPL-3.0-or-later
//! Noise reduction (`doc/m3-multi-user.md` §5): spectral subtraction on
//! the audio.
//!
//! Short-time spectra (256 samples, 21 ms at 12 kS/s, sqrt-Hann windows
//! at 50 % overlap, so analysis + synthesis add back to the input when
//! nothing is changed). The noise power per bin is estimated by minimum
//! statistics: the minimum of the smoothed power over the last ~1.3 s
//! (speech and keying leave gaps where only noise is left), scaled up by
//! the minimum's bias. Each bin is attenuated by the gain
//! 1 - 3 x strength x noise / power, floored, and smoothed over time,
//! which keeps the "musical noise" of plain subtraction down.

use crate::C32;
use rustfft::{Fft, FftPlanner};
use std::collections::VecDeque;
use std::sync::Arc;

const N: usize = 256;
const HOP: usize = N / 2;
const BINS: usize = N / 2 + 1;
/// Power smoothing per frame.
const ALPHA_P: f32 = 0.8;
/// Minimum statistics: sub-windows of frames, kept for the whole window.
const SUB: usize = 16;
const SUBS: usize = 8;
/// The minimum of a smoothed noise power sits below its mean.
const BIAS: f32 = 2.0;
/// Over-subtraction at full strength: noise bins fluctuate around the
/// estimate, and taking more keeps most of them at the floor.
const OVERSUB: f32 = 3.0;
/// Gain smoothing per frame (rises faster than it falls).
const RISE: f32 = 0.7;
const FALL: f32 = 0.3;

pub struct NoiseReduction {
    strength: f32,
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    /// The frame being filled (starts with HOP samples of the last one).
    inbuf: Vec<f32>,
    /// Second half of the previous frame's output, to overlap-add.
    tail: Vec<f32>,
    /// Finished output; one sample leaves per sample in.
    out: VecDeque<f32>,
    power: Vec<f32>,
    /// Running minimum of this sub-window, and the last sub-windows'.
    sub_min: Vec<f32>,
    mins: VecDeque<Vec<f32>>,
    frames: usize,
    gain: Vec<f32>,
    work: Vec<C32>,
    scratch: Vec<C32>,
}

impl NoiseReduction {
    /// Delay added (samples): 21 ms at 12 kS/s.
    pub const DELAY: usize = N;

    /// `strength` 0..1 (0 passes the audio unchanged, but delayed).
    pub fn new(strength: f32) -> Self {
        let mut p = FftPlanner::new();
        let fft = p.plan_fft_forward(N);
        let ifft = p.plan_fft_inverse(N);
        let scratch_len = fft
            .get_inplace_scratch_len()
            .max(ifft.get_inplace_scratch_len());
        // periodic Hann, square-rooted: w^2 at 50 % overlap sums to 1
        let window = (0..N)
            .map(|k| (0.5 - 0.5 * (2.0 * std::f32::consts::PI * k as f32 / N as f32).cos()).sqrt())
            .collect();
        NoiseReduction {
            strength: strength.clamp(0.0, 1.0),
            fft,
            ifft,
            window,
            inbuf: vec![0.0; HOP],
            tail: vec![0.0; HOP],
            out: VecDeque::from(vec![0.0; HOP]),
            power: vec![0.0; BINS],
            sub_min: vec![f32::MAX; BINS],
            mins: VecDeque::new(),
            frames: 0,
            gain: vec![1.0; BINS],
            work: vec![C32::new(0.0, 0.0); N],
            scratch: vec![C32::new(0.0, 0.0); scratch_len],
        }
    }

    pub fn set_strength(&mut self, s: f32) {
        self.strength = s.clamp(0.0, 1.0);
    }

    /// Process `x` in place; the output lags the input by `DELAY`.
    pub fn process(&mut self, x: &mut [f32]) {
        for s in x {
            self.inbuf.push(*s);
            if self.inbuf.len() == N {
                self.frame();
                self.inbuf.drain(..HOP);
            }
            *s = self.out.pop_front().expect("output primed");
        }
    }

    fn frame(&mut self) {
        for ((w, x), win) in self.work.iter_mut().zip(&self.inbuf).zip(&self.window) {
            *w = C32::new(x * win, 0.0);
        }
        self.fft
            .process_with_scratch(&mut self.work, &mut self.scratch);

        // noise estimate: minimum statistics of the smoothed power
        let first = self.frames == 0;
        for b in 0..BINS {
            let p = self.work[b].norm_sqr();
            self.power[b] = if first {
                p
            } else {
                ALPHA_P * self.power[b] + (1.0 - ALPHA_P) * p
            };
            self.sub_min[b] = self.sub_min[b].min(self.power[b]);
        }
        self.frames += 1;
        if self.frames.is_multiple_of(SUB) {
            self.mins
                .push_back(std::mem::replace(&mut self.sub_min, vec![f32::MAX; BINS]));
            if self.mins.len() > SUBS {
                self.mins.pop_front();
            }
        }

        let floor = 1.0 - 0.9 * self.strength; // -20 dB at full strength
        for b in 0..BINS {
            let min = self
                .mins
                .iter()
                .map(|m| m[b])
                .fold(self.sub_min[b], f32::min);
            let noise = BIAS * min;
            let p = self.work[b].norm_sqr().max(1e-30);
            let g = (1.0 - OVERSUB * self.strength * noise / p).max(floor);
            let k = if g > self.gain[b] { RISE } else { FALL };
            self.gain[b] += k * (g - self.gain[b]);
            let g = self.gain[b];
            self.work[b] *= g;
            if b != 0 && b != N / 2 {
                self.work[N - b] *= g;
            }
        }

        self.ifft
            .process_with_scratch(&mut self.work, &mut self.scratch);
        let k = 1.0 / N as f32;
        for i in 0..HOP {
            let y = self.work[i].re * k * self.window[i];
            self.out.push_back(self.tail[i] + y);
        }
        for i in 0..HOP {
            self.tail[i] = self.work[HOP + i].re * k * self.window[HOP + i];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Noise(u64);
    impl Noise {
        fn uniform(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            ((self.0 >> 11) as f64 / (1u64 << 53) as f64) as f32 - 0.5
        }
        /// Roughly gaussian, unit variance.
        fn gauss(&mut self) -> f32 {
            (0..12).map(|_| self.uniform()).sum()
        }
    }

    const FS: f32 = 12_000.0;

    fn tone(k: usize, f: f32, a: f32) -> f32 {
        a * (2.0 * std::f32::consts::PI * f / FS * k as f32).sin()
    }

    /// Power of `x` at `f` (correlation), and the power of the rest.
    fn split(x: &[f32], k0: usize, f: f32) -> (f32, f32) {
        let n = x.len() as f32;
        let (mut c, mut s) = (0.0, 0.0);
        for (i, v) in x.iter().enumerate() {
            let ph = 2.0 * std::f32::consts::PI * f / FS * (k0 + i) as f32;
            c += v * ph.cos();
            s += v * ph.sin();
        }
        let tone = 2.0 * (c * c + s * s) / (n * n);
        let total = x.iter().map(|v| v * v).sum::<f32>() / n;
        (tone, (total - tone).max(0.0))
    }

    /// Power in `lo..hi` Hz (Hann-windowed 1024-point FFTs, averaged).
    fn band(x: &[f32], lo: f32, hi: f32) -> f32 {
        let n = 1024;
        let fft = FftPlanner::new().plan_fft_forward(n);
        let mut acc = 0.0;
        for c in x.chunks_exact(n) {
            let mut b: Vec<C32> = c
                .iter()
                .enumerate()
                .map(|(k, &v)| {
                    let w = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * k as f32 / n as f32).cos();
                    C32::new(v * w, 0.0)
                })
                .collect();
            fft.process(&mut b);
            let k0 = (lo / FS * n as f32) as usize;
            let k1 = (hi / FS * n as f32) as usize;
            acc += b[k0..k1].iter().map(|z| z.norm_sqr()).sum::<f32>();
        }
        acc
    }

    #[test]
    fn zero_strength_passes_the_audio_delayed() {
        let mut nr = NoiseReduction::new(0.0);
        let input: Vec<f32> = (0..5000).map(|k| tone(k, 700.0, 0.5)).collect();
        let mut x = input.clone();
        // odd chunk sizes: the framing is independent of the calls
        for c in x.chunks_mut(97) {
            nr.process(c);
        }
        for k in NoiseReduction::DELAY + N..5000 {
            assert!(
                (x[k] - input[k - NoiseReduction::DELAY]).abs() < 1e-4,
                "{k}"
            );
        }
    }

    #[test]
    fn noise_drops_and_the_tone_stays() {
        let mut rng = Noise(7);
        let sigma = 0.02;
        let amp = 0.05; // tone 10 dB over the noise in 6 kHz
        let n = 12_000 * 6;
        // keyed like CW (300 ms on, 200 ms off): a carrier that never
        // stops would itself become the noise estimate
        let key = |k: usize| (k / 3600) % 5 < 3;
        let input: Vec<f32> = (0..n)
            .map(|k| {
                let t = if k >= n / 2 && key(k) {
                    tone(k, 1000.0, amp)
                } else {
                    0.0
                };
                sigma * rng.gauss() + t
            })
            .collect();
        let mut x = input.clone();
        let mut nr = NoiseReduction::new(1.0);
        for c in x.chunks_mut(240) {
            nr.process(c);
        }
        let d = NoiseReduction::DELAY;
        // noise only (after 1.5 s of learning): at least 10 dB less
        let a = 12_000 * 3 / 2;
        let b = n / 2 - 2400;
        let p_in: f32 = input[a..b].iter().map(|v| v * v).sum();
        let p_out: f32 = x[a + d..b + d].iter().map(|v| v * v).sum();
        let drop = 10.0 * (p_in / p_out).log10();
        assert!(drop > 10.0, "noise down {drop:.1} dB");
        // with the tone: its level within 1 dB
        let a = n / 2 + 12_000;
        let (t_in, _) = split(&input[a..n - d], a, 1000.0);
        let (t_out, _) = split(&x[a + d..n], a, 1000.0);
        let tone_db = 10.0 * (t_out / t_in).log10();
        assert!(tone_db.abs() < 1.0, "tone {tone_db:.2} dB");
        // noise away from the tone (2..5 kHz) while it's keyed: well down
        let band_db = 10.0
            * (band(&input[a..n - d], 2000.0, 5000.0) / band(&x[a + d..n], 2000.0, 5000.0)).log10();
        assert!(band_db > 10.0, "noise beside the tone down {band_db:.1} dB");
    }
}
