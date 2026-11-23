// SPDX-License-Identifier: GPL-3.0-or-later
//! CW synthesis for tests (and training data): keying from text with
//! hand-keying jitter, Farnsworth spacing, raised-cosine edges; the
//! channel (Watterson fading, ITU-R F.1487) and white noise from
//! ionochannel; complex baseband or real audio.

use crate::dsp::Rng;
use crate::morse;
/// The channel's paths, from ionochannel (also its `presets`).
pub use ionochannel::{PathSpec, presets};
use ionochannel::{RealChannel, noise};
use num_complex::Complex32;

pub fn unit_s(wpm: f64) -> f64 {
    1.2 / wpm
}

/// How to key a text.
#[derive(Clone, Debug)]
pub struct Keying {
    pub wpm: f64,
    /// Character and word gaps at this slower speed.
    pub farnsworth: Option<f64>,
    /// Relative s.d. of every element and gap.
    pub jitter: f64,
    /// Dash length in units.
    pub dash: f64,
    /// Silence before and after (s).
    pub lead: f64,
    pub tail: f64,
}

impl Keying {
    pub fn new(wpm: f64) -> Self {
        Keying {
            wpm,
            farnsworth: None,
            jitter: 0.0,
            dash: 3.0,
            lead: 0.3,
            tail: 0.3,
        }
    }
}

/// 0/1 key-down samples at `fs` for `text` (words separated by spaces).
pub fn keying(text: &str, k: &Keying, fs: f64, rng: &mut Rng) -> Vec<f32> {
    let u = unit_s(k.wpm);
    let uf = k.farnsworth.map(unit_s).unwrap_or(u);
    let mut seq: Vec<(f32, f64)> = vec![(0.0, k.lead)];
    let j = |x: f64, rng: &mut Rng| {
        if k.jitter > 0.0 {
            x * (1.0 + k.jitter * rng.normal()).max(0.2)
        } else {
            x
        }
    };
    for (wi, word) in text.to_uppercase().split_whitespace().enumerate() {
        if wi > 0 {
            seq.push((0.0, j(7.0 * uf, rng)));
        }
        for (ci, ch) in morse::chars(word).iter().enumerate() {
            if ci > 0 {
                seq.push((0.0, j(3.0 * uf, rng)));
            }
            let code = morse::encode(ch).unwrap_or("");
            for (ei, e) in code.chars().enumerate() {
                if ei > 0 {
                    seq.push((0.0, j(u, rng)));
                }
                seq.push((1.0, j(if e == '.' { u } else { k.dash * u }, rng)));
            }
        }
    }
    seq.push((0.0, k.tail));
    let mut out = Vec::new();
    for (s, d) in seq {
        let n = (d * fs).round() as usize;
        out.extend(std::iter::repeat_n(s, n));
    }
    out
}

/// Signal conditions.
#[derive(Clone, Debug)]
pub struct Channel {
    /// Carrier frequency (Hz; complex: offset, real: audio frequency).
    pub f0: f64,
    /// Mark power over noise in 2500 Hz (dB); None: no noise.
    pub snr_db: Option<f64>,
    /// Edge rise time (s).
    pub rise: f64,
    /// Rayleigh fading of one path: the spread (2σ, Hz) of its Gaussian
    /// Doppler spectrum (ITU-R F.1487); 0 for none. A fraction of a Hz is
    /// QSB, several Hz auroral flutter.
    pub spread_hz: f64,
    /// Or a whole Watterson channel (delays, scatter and specular
    /// components); used instead of `spread_hz` when not empty.
    pub paths: Vec<PathSpec>,
}

impl Channel {
    pub fn new(f0: f64) -> Self {
        Channel {
            f0,
            snr_db: None,
            rise: 0.005,
            spread_hz: 0.0,
            paths: Vec::new(),
        }
    }

    /// The propagation paths, if the signal fades at all.
    fn paths(&self) -> Option<Vec<PathSpec>> {
        if !self.paths.is_empty() {
            Some(self.paths.clone())
        } else {
            (self.spread_hz > 0.0).then(|| vec![PathSpec::scatter(0.0, 1.0, self.spread_hz, 0.0)])
        }
    }
}

fn shaped(k: &[f32], fs: f64, rise: f64) -> Vec<f32> {
    if rise <= 0.0 {
        return k.to_vec();
    }
    let m = ((rise * fs) as usize).max(1);
    // np.hanning(2m+1), normalised, 'same'
    let n = 2 * m + 1;
    let w: Vec<f64> = (0..n)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64).cos())
        .collect();
    let s: f64 = w.iter().sum();
    (0..k.len())
        .map(|i| {
            let mut acc = 0.0;
            for (j, wj) in w.iter().enumerate() {
                let idx = i as isize + m as isize - j as isize;
                if idx >= 0 && (idx as usize) < k.len() {
                    acc += wj * k[idx as usize] as f64;
                }
            }
            (acc / s) as f32
        })
        .collect()
}

/// A Rayleigh fading gain sequence of `n` samples at `fs` (mean power 1):
/// one scatter path with a Gaussian Doppler spectrum of spread
/// `spread_hz` (2σ, ITU-R F.1487), from ionochannel.
pub fn fading(spread_hz: f64, n: usize, fs: f64, rng: &mut Rng) -> Vec<Complex32> {
    let mut g = vec![Complex32::new(1.0, 0.0); n];
    let p = [PathSpec::scatter(0.0, 1.0, spread_hz, 0.0)];
    ionochannel::Channel::new(fs, &p, rng.next_u64()).process(&mut g);
    g
}

/// `x` through the channel's paths (delays removed, so the length and
/// the timing stay).
fn propagate_c(x: &mut Vec<Complex32>, paths: &[PathSpec], fs: f64, rng: &mut Rng) {
    let mut ch = ionochannel::Channel::new(fs, paths, rng.next_u64());
    let (n, lat) = (x.len(), ch.latency());
    x.resize(n + lat, Complex32::new(0.0, 0.0));
    ch.process(x);
    x.drain(..lat);
}

fn propagate_r(x: &mut Vec<f32>, paths: &[PathSpec], fs: f64, rng: &mut Rng) {
    let mut ch = RealChannel::new(fs, paths, rng.next_u64());
    let (n, lat) = (x.len(), ch.latency());
    x.resize(n + lat, 0.0);
    ch.process(x);
    x.drain(..lat);
}

/// Complex baseband CW (unit mark power) at `fs`.
pub fn complex(text: &str, k: &Keying, ch: &Channel, fs: f64, rng: &mut Rng) -> Vec<Complex32> {
    let key = shaped(&keying(text, k, fs, rng), fs, ch.rise);
    let ph0 = 2.0 * std::f64::consts::PI * rng.uniform();
    let mut x: Vec<Complex32> = key
        .iter()
        .enumerate()
        .map(|(i, &a)| {
            let ph = 2.0 * std::f64::consts::PI * ch.f0 * i as f64 / fs + ph0;
            Complex32::new((a as f64 * ph.cos()) as f32, (a as f64 * ph.sin()) as f32)
        })
        .collect();
    if let Some(p) = ch.paths() {
        propagate_c(&mut x, &p, fs, rng);
    }
    if let Some(snr) = ch.snr_db {
        // per component: half the complex noise power
        let s = (noise::noise_power(1.0, snr, 2500.0, fs, true) / 2.0).sqrt();
        for v in &mut x {
            *v += Complex32::new((s * rng.normal()) as f32, (s * rng.normal()) as f32);
        }
    }
    x
}

/// Real audio CW at `fs`: amplitude `amp`; SNR as for [`complex`] (mark
/// power amp²/2 over noise in 2500 Hz).
pub fn audio(text: &str, k: &Keying, ch: &Channel, fs: f64, amp: f64, rng: &mut Rng) -> Vec<f32> {
    let key = shaped(&keying(text, k, fs, rng), fs, ch.rise);
    let ph0 = 2.0 * std::f64::consts::PI * rng.uniform();
    let mut x: Vec<f32> = key
        .iter()
        .enumerate()
        .map(|(i, &a)| {
            let ph = 2.0 * std::f64::consts::PI * ch.f0 * i as f64 / fs + ph0;
            (amp * a as f64 * ph.cos()) as f32
        })
        .collect();
    if let Some(p) = ch.paths() {
        propagate_r(&mut x, &p, fs, rng);
    }
    if let Some(snr) = ch.snr_db {
        let s = noise::noise_power(amp * amp / 2.0, snr, 2500.0, fs, false).sqrt();
        for v in &mut x {
            *v += (s * rng.normal()) as f32;
        }
    }
    x
}

/// White noise of the same density as [`audio`] would add, for padding.
pub fn audio_noise(n: usize, fs: f64, amp: f64, snr_db: f64, rng: &mut Rng) -> Vec<f32> {
    let s = noise::noise_power(amp * amp / 2.0, snr_db, 2500.0, fs, false).sqrt();
    (0..n).map(|_| (s * rng.normal()) as f32).collect()
}
