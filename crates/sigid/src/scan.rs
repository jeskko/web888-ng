// SPDX-License-Identifier: GPL-3.0-or-later
//! The passband scanner: finds the signals in 12 kS/s audio (power over
//! the floor, smoothed over ~120 Hz, gaps up to ~190 Hz joined, so a
//! voice or a modem is one signal) and identifies them in turn, one every
//! 2 s (each ~0.3-0.5 s of a core in WebAssembly), each one's class
//! probabilities smoothed over its runs. Weak speech shows only its low
//! frequencies: the identifier is trained to name a voice from a window
//! centred anywhere in its band.
//!
//! The identifier's input is made in the frequency domain: one FFT of the
//! last 4.096 s of audio, then for each signal an inverse FFT of the
//! 6 kHz around it: complex baseband at 6 kS/s with nothing of the
//! negative frequencies (no image), the signal at 0 Hz. LSB audio is
//! flipped back (the signal's RF orientation).

use crate::measure::{Measure, iq_len, measure, measure_iq};
use crate::{CLASSES, N, SigId};
use num_complex::Complex32;
use rustfft::FftPlanner;
use std::collections::VecDeque;

/// The audio rate.
pub const AUDIO_RATE: f64 = 12_000.0;
/// Audio samples a window (4.096 s).
const NA: usize = 2 * N;
/// Signals tracked at most.
const MAX: usize = 8;

/// A signal and what it looks like.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub id: u32,
    /// Audio frequency of its centre, and its edges (Hz).
    pub hz: f64,
    pub lo: f64,
    pub hi: f64,
    /// Peak level over the floor (dB, 23 Hz bins).
    pub db: f64,
    /// Smoothed probabilities per class (empty until identified).
    pub probs: Vec<f32>,
    pub runs: u32,
    /// Stream time it was last seen (s).
    seen: f64,
}

impl Found {
    /// The most likely classes, best first.
    pub fn top(&self, n: usize) -> Vec<(&'static str, f32)> {
        let mut v: Vec<(&'static str, f32)> = CLASSES
            .iter()
            .copied()
            .zip(self.probs.iter().copied())
            .collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1));
        v.truncate(n);
        v
    }
}

pub struct Scanner {
    audio: VecDeque<f32>,
    n_in: u64,
    next: u64,
    lo: f64,
    hi: f64,
    /// LSB: the audio's spectrum is the RF's upside down.
    pub lsb: bool,
    found: Vec<Found>,
    next_id: u32,
    turn: usize,
    /// Seconds between identifications.
    pub every: f64,
}

impl Default for Scanner {
    fn default() -> Self {
        Self::new()
    }
}

impl Scanner {
    pub fn new() -> Self {
        Scanner {
            audio: VecDeque::with_capacity(NA + 4096),
            n_in: 0,
            next: NA as u64,
            lo: 200.0,
            hi: 2800.0,
            lsb: false,
            found: Vec::new(),
            next_id: 1,
            turn: 0,
            every: 2.0,
        }
    }

    /// The audio band searched (Hz).
    pub fn set_band(&mut self, lo: f64, hi: f64) {
        self.lo = lo.max(20.0);
        self.hi = hi.min(AUDIO_RATE / 2.0 - 20.0).max(self.lo + 50.0);
        self.found
            .retain(|f| f.hz >= lo - 50.0 && f.hz <= hi + 50.0);
    }

    pub fn reset(&mut self) {
        self.audio.clear();
        self.found.clear();
        self.next = self.n_in + NA as u64;
    }

    pub fn now(&self) -> f64 {
        self.n_in as f64 / AUDIO_RATE
    }

    /// Feed audio; true when the signals changed (an identification ran).
    pub fn push(&mut self, pcm: &[f32]) -> bool {
        self.audio.extend(pcm);
        while self.audio.len() > NA {
            self.audio.pop_front();
        }
        self.n_in += pcm.len() as u64;
        if self.n_in < self.next || self.audio.len() < NA {
            return false;
        }
        self.next = self.n_in + (self.every * AUDIO_RATE) as u64;
        self.run();
        true
    }

    pub fn found(&self) -> &[Found] {
        &self.found
    }

    /// Seconds of audio held (an analysis needs 4.096).
    pub fn held(&self) -> f64 {
        self.audio.len() as f64 / AUDIO_RATE
    }

    /// What is in audio `lo..hi` (Hz): measured, and named at its centre.
    pub fn analyse(&self, lo: f64, hi: f64) -> Option<Analysis> {
        if self.audio.len() < NA {
            return None;
        }
        let x: Vec<f32> = self.audio.iter().copied().collect();
        analyse(&x, (lo, hi), (self.lo, self.hi), self.lsb)
    }

    fn run(&mut self) {
        let x: Vec<Complex32> = self.audio.iter().map(|&v| Complex32::new(v, 0.0)).collect();
        let now = self.now();
        // the signals now: matched to those known by overlap (half the
        // narrower one's width)
        let over = |a: (f64, f64), b: (f64, f64)| {
            (a.1.min(b.1) - a.0.max(b.0)) / (a.1 - a.0).min(b.1 - b.0).max(1.0)
        };
        for (lo, hi, db) in regions(&x, self.lo, self.hi) {
            let hz = 0.5 * (lo + hi);
            if let Some(f) = self
                .found
                .iter_mut()
                .find(|f| f.seen < now && over((f.lo, f.hi), (lo, hi)) > 0.5)
            {
                (f.hz, f.lo, f.hi, f.db, f.seen) = (hz, lo, hi, db, now);
            } else if self.found.len() < MAX {
                self.found.push(Found {
                    id: self.next_id,
                    hz,
                    lo,
                    hi,
                    db,
                    probs: Vec::new(),
                    runs: 0,
                    seen: now,
                });
                self.next_id += 1;
            }
        }
        // one not seen now that overlaps one that was: the same signal
        let cur: Vec<(f64, f64)> = self
            .found
            .iter()
            .filter(|f| f.seen == now)
            .map(|f| (f.lo, f.hi))
            .collect();
        self.found
            .retain(|f| f.seen == now || !cur.iter().any(|&c| over((f.lo, f.hi), c) > 0.3));
        self.found.retain(|f| now - f.seen < 10.0);
        if self.found.is_empty() {
            return;
        }
        // the next one in turn (new ones first)
        let i = self
            .found
            .iter()
            .position(|f| f.runs == 0)
            .unwrap_or_else(|| {
                self.turn = (self.turn + 1) % self.found.len();
                self.turn
            });
        let y = baseband(&x, self.found[i].hz, self.lsb);
        let l = SigId::get().logits(&y);
        let m = l.iter().copied().fold(f32::MIN, f32::max);
        let e: Vec<f32> = l.iter().map(|v| (v - m).exp()).collect();
        let s: f32 = e.iter().sum();
        let p: Vec<f32> = e.iter().map(|v| v / s).collect();
        let f = &mut self.found[i];
        f.probs = if f.probs.is_empty() {
            p
        } else {
            f.probs
                .iter()
                .zip(&p)
                .map(|(a, b)| 0.5 * a + 0.5 * b)
                .collect()
        };
        f.runs += 1;
        self.found.sort_by(|a, b| a.hz.total_cmp(&b.hz));
    }
}

/// A signal measured ([`measure`]) and named by the identifier at its
/// centre (the classes most likely first).
#[derive(Clone, Debug)]
pub struct Analysis {
    pub m: Measure,
    pub top: Vec<(&'static str, f32)>,
}

/// [`Analysis`] of audio `sel` (Hz) in the last 4.096 s of `audio`
/// (12 kS/s; `band`: the receiver's audio passband).
pub fn analyse(audio: &[f32], sel: (f64, f64), band: (f64, f64), lsb: bool) -> Option<Analysis> {
    let m = measure(audio, sel, band)?;
    let x: Vec<Complex32> = audio[audio.len() - NA..]
        .iter()
        .map(|&v| Complex32::new(v, 0.0))
        .collect();
    let top = crate::identify(&baseband(&x, m.centre, lsb));
    Some(Analysis { m, top })
}

/// [`Analysis`] of the last 4.096 s of complex baseband `iq` at `rate`
/// (the receiver's IQ mode, up to ±rate/2); `sel` and `band` in Hz from
/// its centre, as the result's frequencies. The identifier sees the
/// 6 kHz around the signal's centre (a wider signal: part of it).
pub fn analyse_iq(
    iq: &[Complex32],
    rate: f64,
    sel: (f64, f64),
    band: (f64, f64),
) -> Option<Analysis> {
    let m = measure_iq(iq, rate, sel, band)?;
    let n = iq_len(rate);
    let mut a = iq[iq.len() - n..].to_vec();
    FftPlanner::new().plan_fft_forward(n).process(&mut a);
    let y = extract(&a, rate, m.centre);
    let top = crate::identify(&y);
    Some(Analysis { m, top })
}

/// Signals in the band: (lo, hi, peak dB over the floor), strongest first.
fn regions(x: &[Complex32], lo: f64, hi: f64) -> Vec<(f64, f64, f64)> {
    const NF: usize = 512;
    let df = AUDIO_RATE / NF as f64;
    let p = welch(x, NF);
    regions_in(&p[..NF / 2], 0.0, df, lo, hi, MAX)
}

/// Power spectrum averaged over Hann windows of `nf` (half overlap), in
/// FFT order.
pub(crate) fn welch(x: &[Complex32], nf: usize) -> Vec<f64> {
    let fft = FftPlanner::new().plan_fft_forward(nf);
    let w: Vec<f32> = (0..nf)
        .map(|i| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / nf as f64).cos()) as f32)
        .collect();
    let mut p = vec![0f64; nf];
    let mut buf = vec![Complex32::new(0.0, 0.0); nf];
    let mut i = 0;
    while i + nf <= x.len() {
        for k in 0..nf {
            buf[k] = x[i + k] * w[k];
        }
        fft.process(&mut buf);
        for k in 0..nf {
            p[k] += buf[k].norm_sqr() as f64;
        }
        i += nf / 2;
    }
    p
}

/// Signals in power spectrum `p` (bin k at `f0 + k df` Hz) between `lo`
/// and `hi` Hz: (lo, hi, peak dB over the floor), strongest first, at
/// most `max`. The extent from the spectrum smoothed over 5 bins (~117
/// Hz at 23.4 Hz bins: a voice's or modem's band, not its harmonics), 4
/// dB over the floor (the band's 40th percentile), gaps of up to 8 bins
/// (~190 Hz) joined; a signal needs a bin 10 dB up.
pub(crate) fn regions_in(
    p: &[f64],
    f0: f64,
    df: f64,
    lo: f64,
    hi: f64,
    max: usize,
) -> Vec<(f64, f64, f64)> {
    if p.is_empty() {
        return Vec::new();
    }
    let k0 = ((lo - f0) / df).ceil().max(0.0) as usize;
    let k1 = (((hi - f0) / df).floor().max(0.0) as usize).min(p.len() - 1);
    if k1 <= k0 + 4 {
        return Vec::new();
    }
    let db: Vec<f64> = (k0..=k1).map(|k| 10.0 * (p[k] + 1e-30).log10()).collect();
    let sm: Vec<f64> = (0..db.len())
        .map(|j| {
            let (a, b) = (j.saturating_sub(2), (j + 3).min(db.len()));
            10.0 * ((k0 + a..k0 + b).map(|k| p[k]).sum::<f64>() / (b - a) as f64 + 1e-30).log10()
        })
        .collect();
    let mut s = db.clone();
    s.sort_by(|a, b| a.total_cmp(b));
    let floor = s[s.len() * 2 / 5];
    let mut out = Vec::new();
    let mut j = 0;
    while j < sm.len() {
        if sm[j] < floor + 4.0 {
            j += 1;
            continue;
        }
        let a = j;
        let mut b = j;
        let mut gap = 0;
        while j < sm.len() && gap <= 8 {
            if sm[j] >= floor + 4.0 {
                b = j;
                gap = 0;
            } else {
                gap += 1;
            }
            j += 1;
        }
        let peak = db[a..=b].iter().copied().fold(f64::MIN, f64::max) - floor;
        if peak >= 10.0 {
            out.push((
                f0 + ((k0 + a) as f64 - 0.5) * df,
                f0 + ((k0 + b) as f64 + 0.5) * df,
                peak,
            ));
        }
    }
    out.sort_by(|a, b| b.2.total_cmp(&a.2));
    out.truncate(max);
    out
}

/// The identifier's input from the spectrum `a` (FFT order) of 4.096 s
/// of complex baseband at `rate`: the N bins around `centre` Hz (the same
/// bin width as the identifier's: both 4.096 s long), back to N samples
/// at 6 kS/s with `centre` at 0 Hz.
pub(crate) fn extract(a: &[Complex32], rate: f64, centre: f64) -> Vec<Complex32> {
    let n = a.len();
    let df = rate / n as f64;
    let c = (centre / df).round() as i64;
    let mut y = vec![Complex32::new(0.0, 0.0); N];
    for (j, v) in y.iter_mut().enumerate() {
        let k = c + j as i64 - (N / 2) as i64;
        if ((k as f64) * df).abs() < 0.5 * rate {
            *v = a[k.rem_euclid(n as i64) as usize];
        }
    }
    y.rotate_left(N / 2);
    FftPlanner::new().plan_fft_inverse(N).process(&mut y);
    let g = 1.0 / n as f32;
    for v in y.iter_mut() {
        *v *= g;
    }
    y
}

/// The identifier's input: N samples at 6 kS/s, audio frequency `hz` at
/// 0 Hz, from NA samples of 12 kS/s audio (as complex, imaginary 0).
pub fn baseband(x: &[Complex32], hz: f64, lsb: bool) -> Vec<Complex32> {
    let mut planner = FftPlanner::new();
    let mut a = x[x.len() - NA..].to_vec();
    planner.plan_fft_forward(NA).process(&mut a);
    let df = AUDIO_RATE / NA as f64;
    let c = (hz / df).round() as i64;
    let mut y = vec![Complex32::new(0.0, 0.0); N];
    for (j, v) in y.iter_mut().enumerate() {
        // output bin j (0..N, fftshifted below) <- audio bin c + j - N/2
        let k = c + j as i64 - (N / 2) as i64;
        if k > 0 && (k as usize) < NA / 2 {
            *v = a[k as usize];
        }
    }
    // to FFT order (0 Hz first), inverse
    y.rotate_left(N / 2);
    planner.plan_fft_inverse(N).process(&mut y);
    let g = 2.0 / NA as f32;
    for v in y.iter_mut() {
        *v *= g;
        if lsb {
            *v = v.conj();
        }
    }
    y
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(f: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * std::f64::consts::PI * f * i as f64 / AUDIO_RATE).sin() as f32)
            .collect()
    }

    /// A tone at the centre comes out at 0 Hz with no image; its edges
    /// are found.
    #[test]
    fn baseband_and_regions() {
        let x: Vec<Complex32> = tone(1000.0, NA)
            .iter()
            .map(|&v| Complex32::new(v, 0.0))
            .collect();
        let y = baseband(&x, 1000.0, false);
        let d = (y[200] * y[199].conj()).arg();
        assert!(d.abs() < 1e-3, "phase step {d}");
        assert!((y[300].norm() - 1.0).abs() < 0.05, "{}", y[300].norm());
        // 1300 Hz in the audio: +300 Hz in the window (USB), -300 Hz (LSB)
        let x: Vec<Complex32> = tone(1300.0, NA)
            .iter()
            .map(|&v| Complex32::new(v, 0.0))
            .collect();
        for (lsb, want) in [(false, 300.0), (true, -300.0)] {
            let y = baseband(&x, 1000.0, lsb);
            let f = (y[201] * y[200].conj()).arg() as f64 * 6000.0 / (2.0 * std::f64::consts::PI);
            assert!((f - want).abs() < 1.0, "{lsb}: {f}");
        }
        let mut a = tone(700.0, NA);
        let mut rng = 1u64;
        for v in a.iter_mut() {
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *v = 0.1 * *v + 0.01 * ((rng >> 33) as f32 / (1u64 << 31) as f32 - 0.5);
        }
        let x: Vec<Complex32> = a.iter().map(|&v| Complex32::new(v, 0.0)).collect();
        let r = regions(&x, 200.0, 2800.0);
        assert_eq!(r.len(), 1, "{r:?}");
        assert!(
            r[0].0 < 700.0 && r[0].1 > 700.0 && r[0].1 - r[0].0 < 300.0,
            "{r:?}"
        );
    }
}
