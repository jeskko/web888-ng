// SPDX-License-Identifier: GPL-3.0-or-later
//! Automatic notch (`doc/m3-multi-user.md` §5): removes steady carriers
//! (heterodynes, birdies, a carrier beside an SSB signal) from the audio
//! and leaves speech, CW and noise as they are.
//!
//! Short-time spectra (512 samples at 12 kS/s: 23 Hz bins, 43 ms; at
//! 24 kS/s 1024, the same time and resolution), sqrt-Hann windows at 50 %
//! overlap, as the noise reduction. A carrier is a bin whose level (dB)
//! hardly changes from one frame to the next (the mean square of the
//! change over ~0.3 s under `STEADY_DB2`: a fading carrier changes by
//! fractions of a dB in 21 ms, noise alone by ~8 dB) and is well over the
//! local floor (`ABOVE_DB` over the lower quartile of the bins around
//! it). It is brought down to that floor, following its fades, so the
//! noise around it stays and no hole opens. Speech moves from bin to bin
//! and CW keys on and off: their levels jump, and they are never steady
//! long enough. A carrier is caught within about a second and let go in
//! the frame it stops. Several carriers at once are fine, unlike a single
//! adaptive (LMS) notch.

use crate::C32;
use rustfft::Fft;
use std::collections::VecDeque;
use std::sync::Arc;

/// The spectra's length at 12 kS/s.
const N: usize = 512;
/// Smoothing per frame (21 ms) of the floor and of the changes: ~0.3 s.
const ALPHA: f32 = 0.07;
/// Smoothing of the level a notch follows: ~70 ms.
const ALPHA_FAST: f32 = 0.3;
/// A carrier's frame-to-frame change stays under this (mean square,
/// dB²); noise alone changes by ~62 dB².
const STEADY_DB2: f32 = 4.0;
/// How far over the local floor a carrier is (dB).
const ABOVE_DB: f32 = 10.0;
/// The floor: the lower quartile of the bins from `NEAR` to `FAR` away.
const NEAR: usize = 3;
const FAR: usize = 20;
/// The deepest notch.
const MIN_GAIN: f32 = 1e-3;
/// Gain smoothing per frame.
const SMOOTH: f32 = 0.5;

pub struct AutoNotch {
    n: usize,
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    /// The frame being filled (starts with the last one's second half).
    inbuf: Vec<f32>,
    /// Second half of the previous frame's output, to overlap-add.
    tail: Vec<f32>,
    /// Finished output; one sample leaves per sample in.
    out: VecDeque<f32>,
    /// Per bin: the last level, its slow and fast means (dB), the mean
    /// square of its change (dB²), and the gain applied.
    level: Vec<f32>,
    mean: Vec<f32>,
    fast: Vec<f32>,
    change: Vec<f32>,
    gain: Vec<f32>,
    frames: usize,
    near: Vec<f32>,
    work: Vec<C32>,
    scratch: Vec<C32>,
}

impl AutoNotch {
    /// Delay added at 12 kS/s (samples): 43 ms.
    pub const DELAY: usize = N;

    /// At 12 kS/s.
    pub fn new() -> Self {
        Self::at(12_000.0)
    }

    /// At `fs` (a multiple of 12 kS/s: the spectra as long in time).
    pub fn at(fs: f64) -> Self {
        let n = N * ((fs / 12_000.0).round() as usize).max(1);
        let (hop, bins) = (n / 2, n / 2 + 1);
        let fft = crate::fft_plan(n, false);
        let ifft = crate::fft_plan(n, true);
        let scratch_len = fft
            .get_inplace_scratch_len()
            .max(ifft.get_inplace_scratch_len());
        // periodic Hann, square-rooted: w^2 at 50 % overlap sums to 1
        let window = (0..n)
            .map(|k| (0.5 - 0.5 * (2.0 * std::f32::consts::PI * k as f32 / n as f32).cos()).sqrt())
            .collect();
        AutoNotch {
            n,
            fft,
            ifft,
            window,
            inbuf: vec![0.0; hop],
            tail: vec![0.0; hop],
            out: VecDeque::from(vec![0.0; hop]),
            level: vec![0.0; bins],
            mean: vec![0.0; bins],
            fast: vec![0.0; bins],
            change: vec![0.0; bins],
            gain: vec![1.0; bins],
            frames: 0,
            near: Vec::with_capacity(2 * FAR),
            work: vec![C32::new(0.0, 0.0); n],
            scratch: vec![C32::new(0.0, 0.0); scratch_len],
        }
    }

    /// Process `x` in place; the output lags the input by `DELAY` (at
    /// 12 kS/s; twice the samples at 24 kS/s).
    pub fn process(&mut self, x: &mut [f32]) {
        for s in x {
            self.inbuf.push(*s);
            if self.inbuf.len() == self.n {
                self.frame();
                self.inbuf.drain(..self.n / 2);
            }
            *s = self.out.pop_front().expect("output primed");
        }
    }

    /// The bins notched now (for tests).
    #[cfg(test)]
    fn notched(&self) -> Vec<usize> {
        (0..self.gain.len())
            .filter(|&b| self.gain[b] < 0.5)
            .collect()
    }

    /// Whether a carrier is notched now.
    pub fn active(&self) -> bool {
        self.gain.iter().any(|&g| g < 0.5)
    }

    fn frame(&mut self) {
        let (n, hop, bins) = (self.n, self.n / 2, self.n / 2 + 1);
        for ((w, x), win) in self.work.iter_mut().zip(&self.inbuf).zip(&self.window) {
            *w = C32::new(x * win, 0.0);
        }
        self.fft
            .process_with_scratch(&mut self.work, &mut self.scratch);

        let first = self.frames == 0;
        self.frames += 1;
        for b in 0..bins {
            let l = crate::fastdb::db10(self.work[b].norm_sqr() + 1e-20);
            if first {
                self.mean[b] = l;
                self.fast[b] = l;
                // unsettled: nothing is steady yet
                self.change[b] = 100.0;
            } else {
                let d = l - self.level[b];
                self.change[b] += ALPHA * (d * d - self.change[b]);
                self.mean[b] += ALPHA * (l - self.mean[b]);
                self.fast[b] += ALPHA_FAST * (l - self.fast[b]);
            }
            self.level[b] = l;
        }

        // DC and Nyquist are left alone
        for b in 1..bins - 1 {
            let steady = self.change[b] < STEADY_DB2;
            let mut target = 1.0;
            if steady {
                self.near.clear();
                let lo = b.saturating_sub(FAR).max(1);
                let hi = (b + FAR).min(bins - 2);
                for k in lo..=hi {
                    if k.abs_diff(b) >= NEAR {
                        self.near.push(self.mean[k]);
                    }
                }
                if !self.near.is_empty() {
                    let q = self.near.len() / 4;
                    let (_, floor, _) = self.near.select_nth_unstable_by(q, f32::total_cmp);
                    let floor = *floor;
                    let over = self.fast[b] - floor;
                    if over > ABOVE_DB {
                        target = 10f32.powf(-over / 20.0).max(MIN_GAIN);
                    }
                }
            }
            self.gain[b] += SMOOTH * (target - self.gain[b]);
            let g = self.gain[b];
            self.work[b] *= g;
            self.work[n - b] *= g;
        }

        self.ifft
            .process_with_scratch(&mut self.work, &mut self.scratch);
        let k = 1.0 / n as f32;
        for i in 0..hop {
            let y = self.work[i].re * k * self.window[i];
            self.out.push_back(self.tail[i] + y);
        }
        for i in 0..hop {
            self.tail[i] = self.work[hop + i].re * k * self.window[hop + i];
        }
    }
}

impl Default for AutoNotch {
    fn default() -> Self {
        Self::new()
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
    const D: usize = AutoNotch::DELAY;

    fn tone(k: usize, f: f32, a: f32) -> f32 {
        a * (2.0 * std::f32::consts::PI * f / FS * k as f32).sin()
    }

    /// Power of `x` (from sample `k0`) at `f`.
    fn at(x: &[f32], k0: usize, f: f32) -> f32 {
        let n = x.len() as f32;
        let (mut c, mut s) = (0.0f64, 0.0f64);
        for (i, v) in x.iter().enumerate() {
            let ph = 2.0 * std::f64::consts::PI * f as f64 / FS as f64 * (k0 + i) as f64;
            c += *v as f64 * ph.cos();
            s += *v as f64 * ph.sin();
        }
        (2.0 * (c * c + s * s) / (n as f64 * n as f64)) as f32
    }

    fn power(x: &[f32]) -> f32 {
        x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32
    }

    fn run(input: &[f32]) -> Vec<f32> {
        let mut x = input.to_vec();
        let mut an = AutoNotch::new();
        for c in x.chunks_mut(240) {
            an.process(c);
        }
        x
    }

    #[test]
    fn passes_the_audio_delayed_when_nothing_is_steady() {
        let mut rng = Noise(3);
        let input: Vec<f32> = (0..24_000).map(|_| 0.1 * rng.gauss()).collect();
        let mut x = input.clone();
        let mut an = AutoNotch::new();
        // odd chunk sizes: the framing is independent of the calls
        for c in x.chunks_mut(97) {
            an.process(c);
        }
        assert!(an.notched().is_empty(), "{:?}", an.notched());
        let err: f32 = (D + N..24_000)
            .map(|k| (x[k] - input[k - D]).powi(2))
            .sum::<f32>()
            / power(&input);
        assert!(err / ((24_000 - D - N) as f32) < 1e-6, "{err}");
    }

    #[test]
    fn carriers_go_and_the_noise_around_them_stays() {
        let mut rng = Noise(7);
        let n = 12_000 * 4;
        // two heterodynes 30 and 20 dB over the noise in a 23 Hz bin
        let sigma = 0.01;
        let input: Vec<f32> = (0..n)
            .map(|k| sigma * rng.gauss() + tone(k, 1234.5, 0.15) + tone(k, 2100.0, 0.05))
            .collect();
        let x = run(&input);
        // from 1.5 s on
        let a = 18_000;
        for f in [1234.5, 2100.0] {
            let drop = 10.0 * (at(&input[a..n - D], a, f) / at(&x[a + D..n], a, f)).log10();
            assert!(drop > 25.0, "{f} Hz down {drop:.1} dB");
        }
        // what's left is about the noise: within 1 dB of the noise alone
        let left = 10.0 * (power(&x[a + D..n]) / (sigma * sigma)).log10();
        assert!(left.abs() < 1.0, "{left:.2} dB");
    }

    /// A carrier fading by 20 dB at 0.5 Hz (as AM on HF does): followed
    /// through its fades.
    #[test]
    fn a_fading_carrier_goes() {
        let mut rng = Noise(13);
        let n = 12_000 * 6;
        let sigma = 0.002;
        let fade = |k: usize| {
            let t = k as f32 / FS;
            // 0.1 .. 1.0
            10f32.powf(-0.5 * (1.0 + (2.0 * std::f32::consts::PI * 0.5 * t).sin()))
        };
        let input: Vec<f32> = (0..n)
            .map(|k| sigma * rng.gauss() + fade(k) * tone(k, 1500.0, 0.2))
            .collect();
        let x = run(&input);
        let a = 2 * 12_000;
        let drop = 10.0 * (at(&input[a..n - D], a, 1500.0) / at(&x[a + D..n], a, 1500.0)).log10();
        assert!(drop > 25.0, "down {drop:.1} dB");
    }

    #[test]
    fn a_carrier_is_let_go_when_it_stops() {
        let mut rng = Noise(11);
        let n = 12_000 * 4;
        let input: Vec<f32> = (0..n)
            .map(|k| 0.01 * rng.gauss() + if k < n / 2 { tone(k, 800.0, 0.2) } else { 0.0 })
            .collect();
        let mut x = input.clone();
        let mut an = AutoNotch::new();
        for c in x[..n / 2].chunks_mut(240) {
            an.process(c);
        }
        assert!(!an.notched().is_empty());
        // 0.1 s after it stops
        for c in x[n / 2..n / 2 + 1200 + D].chunks_mut(240) {
            an.process(c);
        }
        assert!(an.notched().is_empty(), "{:?}", an.notched());
    }

    /// Voice-like: a 4 Hz syllable rhythm on harmonics of a pitch that
    /// glides, in noise.
    #[test]
    fn speech_and_cw_pass() {
        let mut rng = Noise(5);
        let n = 12_000 * 6;
        let mut ph = 0.0f64;
        let speech: Vec<f32> = (0..n)
            .map(|k| {
                let t = k as f64 / FS as f64;
                let f0 = 140.0 * (1.0 + 0.12 * (2.0 * std::f64::consts::PI * 0.7 * t).sin());
                ph += 2.0 * std::f64::consts::PI * f0 / FS as f64;
                let env = (2.0 * std::f64::consts::PI * 4.0 * t).sin().max(0.0);
                let v: f64 = (1..20).map(|h| (h as f64 * ph).sin() / h as f64).sum();
                (0.1 * env * v) as f32 + 0.005 * rng.gauss()
            })
            .collect();
        // CW at 20 WPM (60 ms dots), 700 Hz
        let cw: Vec<f32> = (0..n)
            .map(|k| {
                let el = k / 720;
                let on = el % 4 == 0 || el % 7 == 2;
                (if on { tone(k, 700.0, 0.2) } else { 0.0 }) + 0.005 * rng.gauss()
            })
            .collect();
        for (name, input) in [("speech", speech), ("cw", cw)] {
            let x = run(&input);
            let a = 12_000;
            let db = 10.0 * (power(&x[a + D..n]) / power(&input[a..n - D])).log10();
            assert!(db.abs() < 0.5, "{name}: {db:.2} dB");
        }
    }

    #[test]
    fn at_24k_as_at_12k() {
        let mut rng = Noise(9);
        let fs = 24_000.0;
        let n = 24_000 * 3;
        let input: Vec<f32> = (0..n)
            .map(|k| {
                0.01 * rng.gauss()
                    + 0.1 * (2.0 * std::f32::consts::PI * 7000.0 / fs * k as f32).sin()
            })
            .collect();
        let mut x = input.clone();
        let mut an = AutoNotch::at(24_000.0);
        for c in x.chunks_mut(480) {
            an.process(c);
        }
        let d = 2 * D;
        let a = 36_000;
        let left = 10.0 * (power(&x[a + d..n]) / 1e-4).log10();
        assert!(left.abs() < 1.0, "{left:.2} dB");
    }
}
