// SPDX-License-Identifier: GPL-3.0-or-later
//! Polyphase channelizer: the software model of the channelizer bitstream
//! (`hw/docs/channelizer.md`). Float reference for now; the fixed-point
//! model and the Verilog follow it.
//!
//! The real ADC signal (122.88 MS/s) goes through a polyphase filter bank
//! of M branches with T taps each and a real FFT every D = M/2 samples:
//! M/2 + 1 bins, `fs/M` apart, each a complex stream at `2 fs/M` (2×
//! oversampled). Bin k is the band around `k fs/M`, flat to `pass_hz` and
//! with everything beyond `stop_hz` rejected, so a channel whose centre is
//! within half a bin of k fits entirely inside bin k ([`PfbChannel`]).
//!
//! Frame m of bin k is `X_k[m] = Σ_n x[n] h[mD - n] e^{-j2πkn/M}`: the
//! input shifted down by `k fs/M`, low-passed by the prototype h and
//! sampled at `fs/D`. With the window ending at sample mD, branch sums
//! `u[r] = Σ_t h[r + tM] x[mD - r - tM]` give
//! `X_k = e^{-j2πkmD/M} Σ_r u[r] e^{j2πkr/M}`, and for D = M/2 the phase
//! factor is (-1)^{km}.
//!
//! Scaling: a full-scale real sine (amplitude 1) gives |X| = 1 in its bin,
//! as a full-scale sine reads 1 on an RX channel (`rx_to_c32`).

use crate::C32;
use crate::audio::{AudioChain, AudioSpec};
use crate::chan::Droop;
use crate::fir::lowpass;
use crate::nco::Shifter;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PfbSpec {
    /// ADC rate (real samples/s).
    pub fs: f64,
    /// M: branches (the FFT size).
    pub branches: usize,
    /// T: prototype taps per branch.
    pub taps: usize,
    /// Flat to ±`pass_hz` around a bin centre.
    pub pass_hz: f64,
    /// Rejected beyond ±`stop_hz`.
    pub stop_hz: f64,
    /// Kaiser window design attenuation.
    pub atten_db: f64,
}

impl PfbSpec {
    /// The proposed Web-888 bank: 4096 branches, 30 kHz bins at 60 kS/s.
    pub fn web888() -> Self {
        PfbSpec {
            fs: 122.88e6,
            branches: 4096,
            taps: 11,
            pass_hz: 21e3,
            stop_hz: 39e3,
            atten_db: 100.0,
        }
    }

    pub fn bin_hz(&self) -> f64 {
        self.fs / self.branches as f64
    }

    /// Each bin's sample rate.
    pub fn out_rate(&self) -> f64 {
        2.0 * self.bin_hz()
    }

    pub fn bins(&self) -> usize {
        self.branches / 2 + 1
    }

    /// The bin a channel at `hz` belongs in (its centre is nearest).
    pub fn bin_of(&self, hz: f64) -> usize {
        ((hz / self.bin_hz()).round().max(0.0) as usize).min(self.bins() - 1)
    }

    /// The prototype low-pass, M × T taps, scaled for |X| = 1 at full scale.
    pub fn prototype(&self) -> Vec<f32> {
        let n = self.branches * self.taps;
        let fc = (self.pass_hz + self.stop_hz) / 2.0 / self.fs;
        lowpass(n, fc, self.atten_db)
            .into_iter()
            .map(|h| (2.0 * h) as f32)
            .collect()
    }
}

/// The analysis filter bank (what the FPGA would compute).
pub struct Pfb {
    spec: PfbSpec,
    h: Vec<f32>,
    /// Input history (at least M × T samples once running).
    buf: Vec<f32>,
    /// Samples since the last frame.
    since: usize,
    frame: u64,
    fft: Arc<dyn Fft<f32>>,
    u: Vec<C32>,
    scratch: Vec<C32>,
    out: Vec<C32>,
}

impl Pfb {
    pub fn new(spec: PfbSpec) -> Self {
        assert!(spec.branches.is_power_of_two() && spec.taps >= 1);
        let fft = FftPlanner::new().plan_fft_forward(spec.branches);
        let scratch = vec![C32::default(); fft.get_inplace_scratch_len()];
        Pfb {
            h: spec.prototype(),
            buf: Vec::new(),
            since: 0,
            frame: 0,
            u: vec![C32::default(); spec.branches],
            out: vec![C32::default(); spec.bins()],
            fft,
            scratch,
            spec,
        }
    }

    pub fn spec(&self) -> &PfbSpec {
        &self.spec
    }

    /// Add ADC samples (full scale ±1); `frame` gets every frame's bins
    /// 0..=M/2.
    pub fn push(&mut self, x: &[f32], mut frame: impl FnMut(&[C32])) {
        let d = self.spec.branches / 2;
        let l = self.spec.branches * self.spec.taps;
        let mut rest = x;
        while !rest.is_empty() {
            let n = (d - self.since).min(rest.len());
            self.buf.extend_from_slice(&rest[..n]);
            rest = &rest[n..];
            self.since += n;
            if self.since == d {
                self.since = 0;
                if self.buf.len() >= l {
                    self.compute();
                    frame(&self.out);
                }
                if self.buf.len() > 4 * l {
                    let k = self.buf.len() - l;
                    self.buf.drain(..k);
                }
            }
        }
    }

    fn compute(&mut self) {
        let m = self.spec.branches;
        let l = m * self.spec.taps;
        let s = &self.buf[self.buf.len() - l..];
        // branch sums; s[l - 1] is the newest sample x[mD]
        for r in 0..m {
            let mut acc = 0.0f32;
            for t in 0..self.spec.taps {
                acc += self.h[r + t * m] * s[l - 1 - r - t * m];
            }
            self.u[r] = C32::new(acc, 0.0);
        }
        self.fft
            .process_with_scratch(&mut self.u, &mut self.scratch);
        let odd = self.frame % 2 == 1;
        for (k, o) in self.out.iter_mut().enumerate() {
            let x = self.u[k].conj();
            *o = if odd && k % 2 == 1 { -x } else { x };
        }
        self.frame += 1;
    }
}

/// The bank for complex input (the iq backend: an SDR's I/Q around a
/// centre frequency): M bins covering the whole input, bin k at offset
/// `k fs/M` from the centre (k >= M/2: negative offsets). Same filter and
/// rates as [`Pfb`]; the phase factor is the same, and the sum over
/// branches is an inverse FFT: `X_k = (-1)^{km} Σ_r u[r] e^{j2πkr/M}`.
/// Scaling: a complex tone of amplitude A gives |X| = A.
pub struct PfbComplex {
    spec: PfbSpec,
    h: Vec<f32>,
    buf: Vec<C32>,
    since: usize,
    frame: u64,
    ifft: Arc<dyn Fft<f32>>,
    u: Vec<C32>,
    scratch: Vec<C32>,
}

impl PfbComplex {
    pub fn new(spec: PfbSpec) -> Self {
        assert!(spec.branches.is_power_of_two() && spec.taps >= 1);
        let ifft = FftPlanner::new().plan_fft_inverse(spec.branches);
        let scratch = vec![C32::default(); ifft.get_inplace_scratch_len()];
        // unity gain for complex input (the real bank doubles it)
        let h = spec.prototype().into_iter().map(|v| v / 2.0).collect();
        PfbComplex {
            h,
            buf: Vec::new(),
            since: 0,
            frame: 0,
            u: vec![C32::default(); spec.branches],
            ifft,
            scratch,
            spec,
        }
    }

    pub fn spec(&self) -> &PfbSpec {
        &self.spec
    }

    /// The bin for an offset from the centre, and the offset from its centre.
    pub fn bin_of(&self, offset_hz: f64) -> (usize, f64) {
        let m = self.spec.branches as i64;
        let k = (offset_hz / self.spec.bin_hz()).round() as i64;
        (
            k.rem_euclid(m) as usize,
            offset_hz - k as f64 * self.spec.bin_hz(),
        )
    }

    /// Add input samples; `frame` gets every frame's M bins.
    pub fn push(&mut self, x: &[C32], mut frame: impl FnMut(&[C32])) {
        let d = self.spec.branches / 2;
        let l = self.spec.branches * self.spec.taps;
        let mut rest = x;
        while !rest.is_empty() {
            let n = (d - self.since).min(rest.len());
            self.buf.extend_from_slice(&rest[..n]);
            rest = &rest[n..];
            self.since += n;
            if self.since == d {
                self.since = 0;
                if self.buf.len() >= l {
                    self.compute();
                    frame(&self.u);
                }
                if self.buf.len() > 4 * l {
                    let k = self.buf.len() - l;
                    self.buf.drain(..k);
                }
            }
        }
    }

    fn compute(&mut self) {
        let m = self.spec.branches;
        let l = m * self.spec.taps;
        let s = &self.buf[self.buf.len() - l..];
        for r in 0..m {
            let mut acc = C32::default();
            for t in 0..self.spec.taps {
                acc += s[l - 1 - r - t * m] * self.h[r + t * m];
            }
            self.u[r] = acc;
        }
        self.ifft
            .process_with_scratch(&mut self.u, &mut self.scratch);
        if self.frame % 2 == 1 {
            for (k, x) in self.u.iter_mut().enumerate() {
                if k % 2 == 1 {
                    *x = -*x;
                }
            }
        }
        self.frame += 1;
    }
}

/// One user's channel from its bin: shift the dial to 0 Hz, then the
/// audio chain at the bin rate (what the CPU would do per user).
pub struct PfbChannel {
    bin: usize,
    shift: Shifter,
    chain: AudioChain,
    x: Vec<C32>,
}

impl PfbChannel {
    pub fn new(spec: &PfbSpec, dial_hz: f64, audio: AudioSpec) -> Self {
        let bin = spec.bin_of(dial_hz);
        Self::at(spec, bin, dial_hz - bin as f64 * spec.bin_hz(), audio)
    }

    /// From bin `bin`, the dial `offset` Hz from its centre.
    pub fn at(spec: &PfbSpec, bin: usize, offset: f64, audio: AudioSpec) -> Self {
        PfbChannel {
            bin,
            shift: Shifter::new(-offset, spec.out_rate()),
            chain: AudioChain::new(audio, spec.out_rate(), Droop::None),
            x: Vec::new(),
        }
    }

    pub fn bin(&self) -> usize {
        self.bin
    }

    /// Samples of the bin (`bin()`), appending audio to `out`.
    pub fn process(&mut self, bin: &[C32], out: &mut Vec<f32>) {
        self.x.clear();
        self.x.extend_from_slice(bin);
        self.shift.process(&mut self.x);
        self.chain.process_c32(self.x.iter().copied(), out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::Mode;

    /// The bin `k` stream for `x` (after the bank has filled).
    fn bin_stream(pfb: &mut Pfb, x: &[f32], k: usize) -> Vec<C32> {
        let mut s = Vec::new();
        pfb.push(x, |f| s.push(f[k]));
        s
    }

    fn tone(fs: f64, f: f64, a: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (a * (2.0 * std::f64::consts::PI * f / fs * i as f64).cos()) as f32)
            .collect()
    }

    /// Amplitude of `s` at frequency `f` (correlation) and the power left.
    fn measure(s: &[C32], f: f64, fs: f64) -> (f64, f64) {
        let n = s.len() as f64;
        let mut acc = num_complex::Complex64::new(0.0, 0.0);
        for (i, z) in s.iter().enumerate() {
            let p = num_complex::Complex64::from_polar(
                1.0,
                -2.0 * std::f64::consts::PI * f / fs * i as f64,
            );
            acc += num_complex::Complex64::new(z.re as f64, z.im as f64) * p;
        }
        let a = acc.norm() / n;
        let total = s.iter().map(|z| z.norm_sqr() as f64).sum::<f64>() / n;
        (a, (total - a * a).max(0.0))
    }

    #[test]
    fn a_tone_lands_in_its_bin_at_its_offset_and_level() {
        let spec = PfbSpec::web888();
        let mut pfb = Pfb::new(spec);
        let f = 7_074_000.0 + 1234.0;
        let k = spec.bin_of(f);
        let off = f - k as f64 * spec.bin_hz();
        let x = tone(spec.fs, f, 0.1, 2048 * 200);
        let s = bin_stream(&mut pfb, &x, k);
        // skip the frames still filling the history
        let s = &s[30..];
        let (a, rest) = measure(s, off, spec.out_rate());
        assert!((20.0 * (a / 0.1).log10()).abs() < 0.01, "level {a}");
        // the float model's own floor (f32 sums over M × T taps)
        assert!(10.0 * (rest / (a * a)).log10() < -80.0, "residue {rest}");
    }

    #[test]
    fn bins_are_flat_across_the_passband() {
        let spec = PfbSpec::web888();
        let k = 300;
        let mut worst: f64 = 0.0;
        for off in [-21e3, -15e3, -7e3, 0.0, 3e3, 11e3, 18e3, 21e3] {
            let mut pfb = Pfb::new(spec);
            let f = k as f64 * spec.bin_hz() + off;
            let s = bin_stream(&mut pfb, &tone(spec.fs, f, 0.5, 2048 * 80), k);
            let (a, _) = measure(&s[30..], off, spec.out_rate());
            worst = worst.max((20.0 * (a / 0.5).log10()).abs());
        }
        assert!(worst < 0.01, "ripple {worst} dB");
    }

    /// What a tone at offset `off` from bin k leaves in bin k's
    /// passband, in dB relative to the tone.
    fn leak_db(spec: PfbSpec, off: f64) -> f64 {
        let k = 300;
        let mut pfb = Pfb::new(spec);
        let f = k as f64 * spec.bin_hz() + off;
        let s = bin_stream(&mut pfb, &tone(spec.fs, f, 0.5, 2048 * 80), k);
        // the alias lands at off folded into ±out_rate/2
        let r = spec.out_rate();
        let alias = off - r * (off / r).round();
        let (a, _) = measure(&s[30..], alias, r);
        20.0 * (a / 0.5).max(1e-12).log10()
    }

    #[test]
    fn signals_beyond_the_stopband_dont_alias_into_the_passband() {
        let spec = PfbSpec::web888();
        // these fold into ±21 kHz at 60 kS/s
        for off in [39e3, 45e3, 60e3, 81e3, -45e3, -75e3] {
            let db = leak_db(spec, off);
            assert!(db < -95.0, "{off} Hz: {db:.1} dB");
        }
    }

    #[test]
    fn the_complex_bank_places_positive_and_negative_offsets() {
        let spec = PfbSpec {
            fs: 7.68e6,
            branches: 256,
            ..PfbSpec::web888()
        };
        for off in [1_234_567.0, -2_500_100.0, 14_000.0] {
            let mut pfb = PfbComplex::new(spec);
            let (k, rest) = pfb.bin_of(off);
            let w = 2.0 * std::f64::consts::PI * off / spec.fs;
            let x: Vec<C32> = (0..128 * 120)
                .map(|i| {
                    let p = w * i as f64;
                    C32::new((0.3 * p.cos()) as f32, (0.3 * p.sin()) as f32)
                })
                .collect();
            let mut s = Vec::new();
            pfb.push(&x, |f| s.push(f[k]));
            let (a, residue) = measure(&s[30..], rest, spec.out_rate());
            assert!((20.0 * (a / 0.3).log10()).abs() < 0.01, "{off}: level {a}");
            assert!(10.0 * (residue / (a * a)).log10() < -80.0, "{off}: residue");
        }
    }

    #[test]
    fn a_user_channel_from_its_bin_hears_its_signal() {
        let spec = PfbSpec::web888();
        let mut pfb = Pfb::new(spec);
        let dial = 14_074_000.0;
        let mut ch = PfbChannel::new(&spec, dial, AudioSpec::new(Mode::Usb));
        let k = ch.bin();
        // USB tone 1 kHz above the dial, -40 dBFS, 0.5 s in 2048 × 64 chunks
        let w = 2.0 * std::f64::consts::PI * (dial + 1000.0) / spec.fs;
        let chunk = 2048 * 64;
        let mut audio = Vec::new();
        let mut s = Vec::new();
        for c in 0..(spec.fs * 0.5) as usize / chunk {
            let x: Vec<f32> = (0..chunk)
                .map(|i| (0.01 * (w * (c * chunk + i) as f64).cos()) as f32)
                .collect();
            s.clear();
            pfb.push(&x, |f| s.push(f[k]));
            ch.process(&s, &mut audio);
        }
        let a = &audio[audio.len() / 2..];
        let fs = crate::audio::AUDIO_RATE;
        let (amp, rest) = measure(
            &a.iter().map(|&v| C32::new(v, 0.0)).collect::<Vec<_>>(),
            1000.0,
            fs,
        );
        // a real tone: half its power at +1 kHz; the rest is the image
        let snr = 10.0 * ((amp * amp) / (rest - amp * amp).max(1e-30)).log10();
        assert!(amp > 0.05, "audio level {amp}");
        assert!(snr > 30.0, "snr {snr:.1} dB");
    }
}
