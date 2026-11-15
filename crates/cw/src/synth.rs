// SPDX-License-Identifier: GPL-3.0-or-later
//! CW synthesis for tests (and training data): keying from text with
//! hand-keying jitter, Farnsworth spacing, raised-cosine edges, Rayleigh
//! fading and white noise; complex baseband or real audio.

use crate::dsp::Rng;
use crate::morse;
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
    /// Rayleigh fading Doppler spread (Hz), 0 for none.
    pub fade_hz: f64,
    /// Or a Gaussian Doppler spectrum (ITU-R F.1487: auroral flutter) of
    /// this spread (2σ, Hz); used when > 0.
    pub spread_hz: f64,
}

impl Channel {
    pub fn new(f0: f64) -> Self {
        Channel {
            f0,
            snr_db: None,
            rise: 0.005,
            fade_hz: 0.0,
            spread_hz: 0.0,
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

/// Jakes-style Rayleigh gain (16 paths): slow QSB at a fraction of a Hz,
/// auroral flutter at several Hz.
pub struct Fading {
    hz: f64,
    paths: Vec<(f64, f64)>,
}

impl Fading {
    pub fn new(hz: f64, rng: &mut Rng) -> Self {
        let two_pi = 2.0 * std::f64::consts::PI;
        Fading {
            hz,
            paths: (0..16)
                .map(|_| ((two_pi * rng.uniform()).cos(), two_pi * rng.uniform()))
                .collect(),
        }
    }

    /// Gaussian Doppler spectrum of spread `spread` Hz (2σ, as ITU-R
    /// F.1487): the paths' frequencies drawn from it.
    pub fn gaussian(spread: f64, rng: &mut Rng) -> Self {
        let two_pi = 2.0 * std::f64::consts::PI;
        Fading {
            hz: 1.0,
            paths: (0..16)
                .map(|_| (spread / 2.0 * rng.normal(), two_pi * rng.uniform()))
                .collect(),
        }
    }

    /// A channel's fading, if any.
    fn of(ch: &Channel, rng: &mut Rng) -> Option<Self> {
        if ch.spread_hz > 0.0 {
            Some(Self::gaussian(ch.spread_hz, rng))
        } else {
            (ch.fade_hz > 0.0).then(|| Self::new(ch.fade_hz, rng))
        }
    }

    /// The complex gain at `t` s (mean power 1).
    pub fn gain(&self, t: f64) -> (f64, f64) {
        let (mut re, mut im) = (0.0, 0.0);
        for (c, ph) in &self.paths {
            let a = 2.0 * std::f64::consts::PI * self.hz * c * t + ph;
            re += a.cos();
            im += a.sin();
        }
        (re / 4.0, im / 4.0)
    }
}

/// Complex baseband CW (unit mark power) at `fs`.
pub fn complex(text: &str, k: &Keying, ch: &Channel, fs: f64, rng: &mut Rng) -> Vec<Complex32> {
    let key = shaped(&keying(text, k, fs, rng), fs, ch.rise);
    let ph0 = 2.0 * std::f64::consts::PI * rng.uniform();
    let fade = Fading::of(ch, rng);
    let sig = ch
        .snr_db
        .map(|snr| (10f64.powf(-snr / 10.0) / 2500.0 * fs / 2.0).sqrt());
    key.iter()
        .enumerate()
        .map(|(i, &a)| {
            let t = i as f64 / fs;
            let ph = 2.0 * std::f64::consts::PI * ch.f0 * t + ph0;
            let mut v = Complex32::new((a as f64 * ph.cos()) as f32, (a as f64 * ph.sin()) as f32);
            if let Some(f) = &fade {
                let (gr, gi) = f.gain(t);
                v *= Complex32::new(gr as f32, gi as f32);
            }
            if let Some(s) = sig {
                v += Complex32::new((s * rng.normal()) as f32, (s * rng.normal()) as f32);
            }
            v
        })
        .collect()
}

/// Real audio CW at `fs`: amplitude `amp`; SNR as for [`complex`] (mark
/// power amp²/2 over noise in 2500 Hz).
pub fn audio(text: &str, k: &Keying, ch: &Channel, fs: f64, amp: f64, rng: &mut Rng) -> Vec<f32> {
    let key = shaped(&keying(text, k, fs, rng), fs, ch.rise);
    let ph0 = 2.0 * std::f64::consts::PI * rng.uniform();
    let fade = Fading::of(ch, rng);
    // noise s.d. so that its power in 2500 Hz is amp²/2 / snr: PSD (one-sided) * 2500
    let sig = ch
        .snr_db
        .map(|snr| (amp * amp / 2.0 * 10f64.powf(-snr / 10.0) / 2500.0 * fs / 2.0).sqrt());
    key.iter()
        .enumerate()
        .map(|(i, &a)| {
            let t = i as f64 / fs;
            let ph = 2.0 * std::f64::consts::PI * ch.f0 * t + ph0;
            let mut v = if let Some(f) = &fade {
                let (gr, gi) = f.gain(t);
                amp * a as f64 * (gr * ph.cos() - gi * ph.sin())
            } else {
                amp * a as f64 * ph.cos()
            };
            if let Some(s) = sig {
                v += s * rng.normal();
            }
            v as f32
        })
        .collect()
}

/// White noise of the same density as [`audio`] would add, for padding.
pub fn audio_noise(n: usize, fs: f64, amp: f64, snr_db: f64, rng: &mut Rng) -> Vec<f32> {
    let s = (amp * amp / 2.0 * 10f64.powf(-snr_db / 10.0) / 2500.0 * fs / 2.0).sqrt();
    (0..n).map(|_| (s * rng.normal()) as f32).collect()
}
