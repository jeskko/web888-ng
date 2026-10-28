// SPDX-License-Identifier: GPL-3.0-or-later
//! Demodulators. Input is the channel-filtered complex baseband.

use crate::C32;

/// SSB / CW: the channel filter selects the sideband, so the audio is the
/// real part (USB: positive offsets, LSB: negative offsets).
pub fn ssb(input: &[C32], out: &mut Vec<f32>) {
    out.extend(input.iter().map(|z| z.re));
}

/// AM envelope detector with a DC (carrier) blocker.
pub struct Am {
    dc: f32,
    alpha: f32,
}

impl Am {
    /// `fs`: sample rate; the carrier tracker has a ~10 Hz corner.
    pub fn new(fs: f64) -> Self {
        Am {
            dc: 0.0,
            alpha: (1.0 - (-2.0 * std::f64::consts::PI * 10.0 / fs).exp()) as f32,
        }
    }

    /// Carrier level estimate (envelope mean).
    pub fn carrier(&self) -> f32 {
        self.dc
    }

    pub fn process(&mut self, input: &[C32], out: &mut Vec<f32>) {
        for z in input {
            let e = z.norm();
            self.dc += self.alpha * (e - self.dc);
            out.push(e - self.dc);
        }
    }

    /// Carrier-referenced AGC: the audio relative to the carrier, scaled so
    /// that 100 % modulation peaks at `target`. The modulation depth is
    /// kept (a peak AGC would bring every signal to the same loudness);
    /// `min_carrier` caps the gain.
    pub fn process_carrier_agc(
        &mut self,
        input: &[C32],
        out: &mut Vec<f32>,
        target: f32,
        min_carrier: f32,
    ) {
        for z in input {
            let e = z.norm();
            if self.dc == 0.0 {
                self.dc = e; // start on the carrier, not from silence
            }
            self.dc += self.alpha * (e - self.dc);
            out.push(((e - self.dc) / self.dc.max(min_carrier) * target).clamp(-1.0, 1.0));
        }
    }
}

/// What a synchronous detector outputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncOut {
    /// The in-phase part: both sidebands (SAM).
    Both,
    /// One sideband, the channel filter having removed the other (SAL,
    /// SAR): the in-phase part doubled, as loud as both.
    Single,
    /// Stereo, interleaved: the lower sideband left, the upper right
    /// (SAS), split with a Hilbert transformer.
    Stereo,
}

/// Synchronous AM detector: a phase-locked loop on the carrier, the
/// signal turned so the carrier is real, then its in-phase part (the
/// modulation, free of the envelope detector's selective-fading
/// distortion). Second order loop, atan2 phase detector; natural
/// frequency 200 rad/s, damping 0.65 (as WDSP's medium setting), its
/// frequency within `PULL_HZ` of the dial. The phase error is low-passed
/// at `ERR_HZ` first: modulation in one sideband only (or an interferer
/// beside the carrier) reaches the detector, and the loop would turn it
/// into phase jitter that puts it into both sidebands.
pub struct Sync {
    phase: f64,
    /// Loop frequency, rad/sample.
    freq: f64,
    kp: f64,
    ki: f64,
    max_freq: f64,
    err: f64,
    err_alpha: f64,
    /// Carrier level (the in-phase part's mean).
    dc: f32,
    alpha: f32,
    /// The Hilbert transformer's nonzero taps (every other one, from the
    /// first).
    hilbert: Vec<f32>,
    /// The in-phase and quadrature parts' last `HILBERT_LEN` samples, each
    /// written twice (a ring read as one slice at `pos + 1..`).
    ih: Vec<f32>,
    qh: Vec<f32>,
    pos: usize,
}

/// The loop follows a carrier this far (Hz) from the dial.
pub const PULL_HZ: f64 = 500.0;
/// Corner of the phase error's low-pass (well above the loop's ~30 Hz).
const ERR_HZ: f64 = 150.0;
/// Hilbert transformer length (odd): good from about 150 Hz at 12 kS/s.
const HILBERT_LEN: usize = 255;

impl Sync {
    pub fn new(fs: f64) -> Self {
        let (wn, zeta) = (200.0 / fs, 0.65);
        // ideal Hilbert transformer 2 / (pi n) at odd n from the middle
        // (the even taps, as m is odd), Blackman windowed
        let m = HILBERT_LEN / 2;
        let hilbert = (0..HILBERT_LEN)
            .step_by(2)
            .map(|k| {
                let n = k as f64 - m as f64;
                let t = std::f64::consts::TAU * k as f64 / (HILBERT_LEN - 1) as f64;
                let w = 0.42 - 0.5 * t.cos() + 0.08 * (2.0 * t).cos();
                (2.0 / (std::f64::consts::PI * n) * w) as f32
            })
            .collect();
        Sync {
            phase: 0.0,
            freq: 0.0,
            kp: 2.0 * zeta * wn,
            ki: wn * wn,
            max_freq: 2.0 * std::f64::consts::PI * PULL_HZ / fs,
            err: 0.0,
            err_alpha: 1.0 - (-std::f64::consts::TAU * ERR_HZ / fs).exp(),
            dc: 0.0,
            alpha: (1.0 - (-2.0 * std::f64::consts::PI * 10.0 / fs).exp()) as f32,
            hilbert,
            ih: vec![0.0; 2 * HILBERT_LEN],
            qh: vec![0.0; 2 * HILBERT_LEN],
            pos: 0,
        }
    }

    /// The carrier's offset from the dial the loop follows (Hz).
    pub fn offset_hz(&self, fs: f64) -> f64 {
        self.freq * fs / (2.0 * std::f64::consts::PI)
    }

    /// Demodulate into `out` (interleaved L, R for `Stereo`). With
    /// `agc = Some((target, min_carrier))` the audio is relative to the
    /// carrier, 100 % modulation peaking at `target` (as
    /// `Am::process_carrier_agc`); without, the in-phase part less the
    /// carrier.
    pub fn process(
        &mut self,
        input: &[C32],
        kind: SyncOut,
        agc: Option<(f32, f32)>,
        out: &mut Vec<f32>,
    ) {
        let m = HILBERT_LEN / 2;
        let gain = if kind == SyncOut::Single { 2.0 } else { 1.0 };
        for z in input {
            let (s, c) = self.phase.sin_cos();
            let (s, c) = (s as f32, c as f32);
            // z turned back by the loop's phase
            let i = z.re * c + z.im * s;
            let q = z.im * c - z.re * s;
            self.err += self.err_alpha * ((q as f64).atan2(i as f64) - self.err);
            let err = self.err;
            self.freq = (self.freq + self.ki * err).clamp(-self.max_freq, self.max_freq);
            self.phase = (self.phase + self.freq + self.kp * err) % std::f64::consts::TAU;

            if self.dc == 0.0 && agc.is_some() {
                self.dc = i.abs(); // start on the carrier, not from silence
            }
            self.dc += self.alpha * (i - self.dc);
            let scale = match agc {
                Some((target, min_carrier)) => target / self.dc.abs().max(min_carrier),
                None => 1.0,
            };
            let lim = |x: f32| if agc.is_some() { x.clamp(-1.0, 1.0) } else { x };
            match kind {
                SyncOut::Both | SyncOut::Single => out.push(lim((i - self.dc) * gain * scale)),
                SyncOut::Stereo => {
                    let p = self.pos;
                    self.ih[p] = i;
                    self.ih[p + HILBERT_LEN] = i;
                    self.qh[p] = q;
                    self.qh[p + HILBERT_LEN] = q;
                    self.pos = (p + 1) % HILBERT_LEN;
                    // oldest first; output for the middle sample
                    let qw = &self.qh[p + 1..p + 1 + HILBERT_LEN];
                    let hq: f32 = self
                        .hilbert
                        .iter()
                        .zip(qw.iter().rev().step_by(2))
                        .map(|(h, x)| h * x)
                        .sum();
                    let id = self.ih[p + 1 + m] - self.dc;
                    out.push(lim((id + hq) * scale)); // lower sideband
                    out.push(lim((id - hq) * scale)); // upper sideband
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 12_000.0;

    /// Amplitude of the tone at `f` (Hann-windowed DFT).
    fn amp(x: &[f32], f: f64) -> f64 {
        let n = x.len();
        let (mut re, mut im, mut ws) = (0.0, 0.0, 0.0);
        for (k, &v) in x.iter().enumerate() {
            let w = 0.5 - 0.5 * (std::f64::consts::TAU * k as f64 / (n - 1) as f64).cos();
            let a = std::f64::consts::TAU * f * k as f64 / FS;
            re += v as f64 * w * a.cos();
            im -= v as f64 * w * a.sin();
            ws += w;
        }
        2.0 * (re * re + im * im).sqrt() / ws
    }

    /// `secs` of a carrier `off` Hz from the dial with tones at the given
    /// offsets from it (amplitude relative to the carrier's 0.1).
    fn signal(off: f64, tones: &[(f64, f64)], secs: f64) -> Vec<C32> {
        (0..(secs * FS) as usize)
            .map(|k| {
                let t = k as f64 / FS;
                let ph = |f: f64| {
                    C32::from_polar(1.0, (std::f64::consts::TAU * (off + f) * t + 0.7) as f32)
                };
                tones
                    .iter()
                    .fold(ph(0.0) * 0.1, |z, &(f, a)| z + ph(f) * (0.1 * a) as f32)
            })
            .collect()
    }

    #[test]
    fn locks_on_an_offset_carrier_and_demodulates() {
        // 50 % AM (each sideband 25 %), carrier 40 Hz off
        let x = signal(40.0, &[(1000.0, 0.25), (-1000.0, 0.25)], 2.0);
        let mut s = Sync::new(FS);
        let mut out = vec![];
        s.process(&x, SyncOut::Both, Some((0.3, 1e-4)), &mut out);
        assert!((s.offset_hz(FS) - 40.0).abs() < 0.5, "{}", s.offset_hz(FS));
        let a = amp(&out[12_000..], 1000.0);
        assert!((a / 0.15 - 1.0).abs() < 0.03, "{a}");
        // locked: no beat at the offset
        assert!(amp(&out[12_000..], 40.0) < 0.003);
    }

    #[test]
    fn stereo_puts_the_lower_sideband_left() {
        let x = signal(-25.0, &[(1500.0, 0.3), (-700.0, 0.3)], 2.0);
        let mut s = Sync::new(FS);
        let mut out = vec![];
        s.process(&x, SyncOut::Stereo, Some((0.3, 1e-4)), &mut out);
        let (l, r): (Vec<f32>, Vec<f32>) = out[24_000..].chunks(2).map(|f| (f[0], f[1])).unzip();
        let db = |a: f64, b: f64| 20.0 * (a / b).log10();
        // a sideband alone, doubled (as a double-sideband tone of the
        // same depth would be): 2 x 0.3 of the carrier, x target
        assert!(
            (amp(&l, 700.0) / 0.18 - 1.0).abs() < 0.05,
            "{}",
            amp(&l, 700.0)
        );
        assert!(
            (amp(&r, 1500.0) / 0.18 - 1.0).abs() < 0.05,
            "{}",
            amp(&r, 1500.0)
        );
        // what crosses over is the loop's residual jitter from the
        // one-sided tones (more at the lower one): 50 and 39 dB
        assert!(db(amp(&l, 700.0), amp(&l, 1500.0)) > 45.0);
        assert!(db(amp(&r, 1500.0), amp(&r, 700.0)) > 35.0);
    }
}
