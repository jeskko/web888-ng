// SPDX-License-Identifier: GPL-3.0-or-later
//! Waterfall lines from a WF slot's samples (`design.md` §5.2).
//!
//! Each line is the mean power of all FFTs taken during its interval
//! (Welch: Blackman-Harris window, 50 % overlap), not one snapshot, so
//! impulsive noise shows as the steady raised floor it sounds like. The
//! engine streams: FFTs are taken every `stride` samples as the samples
//! arrive. The stride is the hop (N/2), or wider when the previous line had
//! more samples than the FFT budget covers, so the budget is spread over
//! the whole interval instead of its start. A line with no FFT of its own
//! (narrow spans: fewer than N new samples) reuses the newest N samples.
//!
//! Levels are dBFS: a full-scale complex tone reads 0 dB in its bin, with
//! the chain's CIC droop corrected per bin. Merging FFT bins into display
//! bins averages power. Input is a WF slot's samples, or an RX channel's
//! (narrow views of the channel a user listens to, `doc/m3-multi-user.md`
//! §3).

use crate::chan::Droop;
use crate::{C32, rx_to_c32};
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;
use web888_radio::{Iq16, Iq32, WF_SINE_FULL_SCALE};

/// Level of quantised value 0 and the step per value (`quantize`).
pub const WF_DB0: f32 = -160.0;
pub const WF_DB_STEP: f32 = 160.0 / 255.0;
/// Largest droop correction (the CIC response near the span edges).
const MAX_CORRECTION_DB: f64 = 20.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WfSpec {
    /// Sample rate (= span).
    pub fs: f64,
    /// Display bins per line (power of two, <= 8192).
    pub bins: usize,
    /// Most FFTs per line.
    pub budget: usize,
    /// Line rate, for the stride before the first line is measured.
    pub fps: f64,
    /// The chain's CIC response to correct.
    pub droop: Droop,
    /// FFT bins per display bin, at least (2; 1 where the bins are already
    /// a crop of a wider capture, so the FFT isn't twice as big).
    pub fft_per_bin: usize,
}

/// FFT size for `bins` display bins, `per` FFT bins per display bin.
pub fn fft_size(bins: usize, per: usize) -> usize {
    (per.max(1) * bins).next_power_of_two().clamp(1024, 8192)
}

#[derive(Clone, Debug, PartialEq)]
pub struct WfLine {
    /// Level per display bin (dBFS).
    pub db: Vec<f32>,
    /// Centre of the first bin, relative to the WF centre frequency.
    pub first_offset_hz: f64,
    pub bin_hz: f64,
    /// FFTs averaged into this line.
    pub ffts: usize,
    /// Samples arrived since the previous line (else it repeats the
    /// newest ones).
    pub fresh: bool,
}

/// Quantise levels for the wire: `WF_DB0 + v * WF_DB_STEP`.
pub fn quantize(db: &[f32]) -> Vec<u8> {
    db.iter()
        .map(|d| ((d - WF_DB0) / WF_DB_STEP).round().clamp(0.0, 255.0) as u8)
        .collect()
}

pub struct WfEngine {
    spec: WfSpec,
    n: usize,
    hop: usize,
    stride: usize,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    /// Power scale per FFT bin (natural order): full-scale and droop.
    scale: Vec<f32>,
    buf: Vec<C32>,
    /// Start of the next FFT in `buf`.
    next: usize,
    acc: Vec<f32>,
    nacc: usize,
    /// Samples since the last line (stride measurement).
    fresh: usize,
    work: Vec<C32>,
    scratch: Vec<C32>,
}

impl WfEngine {
    pub fn new(spec: WfSpec) -> Self {
        Self::with_fft_size(spec, fft_size(spec.bins, spec.fft_per_bin))
    }

    /// With FFT size `n` (a power of two, at least `spec.bins`), smaller
    /// than [`fft_size`] allows: the per-bin FFTs of a view from a
    /// channelizer's bins.
    pub fn with_fft_size(spec: WfSpec, n: usize) -> Self {
        assert!(spec.bins.is_power_of_two() && spec.bins <= 8192);
        assert!(spec.budget >= 1);
        assert!(n.is_power_of_two() && n >= spec.bins);
        let fft = FftPlanner::new().plan_fft_forward(n);
        // 4-term Blackman-Harris
        let (a0, a1, a2, a3) = (0.35875, 0.48829, 0.14128, 0.01168);
        let window: Vec<f32> = (0..n)
            .map(|k| {
                let x = 2.0 * std::f64::consts::PI * k as f64 / n as f64;
                (a0 - a1 * x.cos() + a2 * (2.0 * x).cos() - a3 * (3.0 * x).cos()) as f32
            })
            .collect();
        let sum_w: f64 = window.iter().map(|&w| w as f64).sum();
        let scale = (0..n)
            .map(|k| {
                let kk = if k < n / 2 {
                    k as f64
                } else {
                    k as f64 - n as f64
                };
                let m = spec.droop.mag(kk * spec.fs / n as f64);
                let corr = (1.0 / (m * m)).min(10f64.powf(MAX_CORRECTION_DB / 10.0));
                (corr / (sum_w * sum_w)) as f32
            })
            .collect();
        let hop = n / 2;
        let mut e = WfEngine {
            spec,
            n,
            hop,
            stride: hop,
            fft: fft.clone(),
            window,
            scale,
            buf: Vec::new(),
            next: 0,
            acc: vec![0.0; n],
            nacc: 0,
            fresh: 0,
            work: vec![C32::new(0.0, 0.0); n],
            scratch: vec![C32::new(0.0, 0.0); fft.get_inplace_scratch_len()],
        };
        e.set_stride((spec.fs / spec.fps) as usize);
        e
    }

    pub fn spec(&self) -> &WfSpec {
        &self.spec
    }

    pub fn fft_n(&self) -> usize {
        self.n
    }

    /// Most FFTs per line from the next line on (load shedding).
    pub fn set_budget(&mut self, budget: usize) {
        self.spec.budget = budget.max(1);
    }

    fn set_stride(&mut self, samples_per_line: usize) {
        self.stride = (samples_per_line / self.spec.budget).max(self.hop);
    }

    /// Add WF slot samples; `contiguous = false` if there was a gap
    /// before them (no FFT spans a gap).
    pub fn push(&mut self, iq: &[Iq16], contiguous: bool) {
        let k = (1.0 / WF_SINE_FULL_SCALE) as f32;
        self.push_c32(Wf16(iq.iter(), k), contiguous);
    }

    /// Add RX channel samples (as `push`).
    pub fn push_rx(&mut self, iq: impl Iterator<Item = Iq32>, contiguous: bool) {
        self.push_c32(iq.map(rx_to_c32), contiguous);
    }

    /// Add samples scaled so that a full-scale ADC sine has |z| = 1.
    /// Samples between FFTs (stride > N) are skipped, not converted.
    pub fn push_c32(&mut self, mut iq: impl Iterator<Item = C32>, contiguous: bool) {
        if !contiguous {
            self.buf.clear();
            self.next = 0;
        }
        loop {
            // before the next FFT and not needed by this line's fallback
            // (it has an FFT already): drop
            if self.nacc > 0 && self.next > self.buf.len() {
                self.next -= self.buf.len();
                self.buf.clear();
                let want = self.next;
                let got = skip(&mut iq, want);
                self.fresh += got;
                self.next -= got;
                if got < want {
                    break;
                }
            }
            let want = self.next + self.n - self.buf.len().min(self.next + self.n);
            let before = self.buf.len();
            self.buf.extend(iq.by_ref().take(want));
            let got = self.buf.len() - before;
            self.fresh += got;
            while self.next + self.n <= self.buf.len() {
                if self.nacc < self.spec.budget {
                    self.fft_at(self.next);
                }
                self.next += self.stride;
            }
            if got < want || want == 0 {
                break;
            }
        }
        // keep the newest N samples (narrow-span fallback) and the next FFT's
        let keep = self.next.min(self.buf.len().saturating_sub(self.n));
        self.buf.drain(..keep);
        self.next -= keep;
    }

    fn fft_at(&mut self, start: usize) {
        for ((w, x), win) in self
            .work
            .iter_mut()
            .zip(&self.buf[start..start + self.n])
            .zip(&self.window)
        {
            *w = x * win;
        }
        self.fft
            .process_with_scratch(&mut self.work, &mut self.scratch);
        for (a, x) in self.acc.iter_mut().zip(&self.work) {
            *a += x.norm_sqr();
        }
        self.nacc += 1;
    }

    /// Finish the current line. `None` until the first N samples arrived.
    pub fn line(&mut self) -> Option<WfLine> {
        let fresh = self.fresh > 0;
        if self.nacc == 0 {
            if self.buf.len() < self.n {
                return None;
            }
            self.fft_at(self.buf.len() - self.n);
        }
        let n = self.n;
        let g = n / self.spec.bins;
        let inv = 1.0 / (self.nacc as f32 * g as f32);
        // fftshift while merging: display bin 0 starts at FFT bin n/2
        let db = (0..self.spec.bins)
            .map(|b| {
                let p: f32 = (0..g)
                    .map(|j| {
                        let k = (n / 2 + b * g + j) % n;
                        self.acc[k] * self.scale[k]
                    })
                    .sum();
                10.0 * (p * inv).max(1e-30).log10()
            })
            .collect();
        let fft_bin = self.spec.fs / n as f64;
        let line = WfLine {
            db,
            first_offset_hz: (-(n as f64) / 2.0 + (g as f64 - 1.0) / 2.0) * fft_bin,
            bin_hz: fft_bin * g as f64,
            ffts: self.nacc,
            fresh,
        };
        self.acc.fill(0.0);
        self.nacc = 0;
        self.set_stride(self.fresh);
        self.fresh = 0;
        Some(line)
    }
}

/// WF slot samples to `C32`, skipping without converting.
struct Wf16<'a>(std::slice::Iter<'a, Iq16>, f32);

impl Iterator for Wf16<'_> {
    type Item = C32;
    fn next(&mut self) -> Option<C32> {
        let s = self.0.next()?;
        Some(C32::new(s.i as f32 * self.1, s.q as f32 * self.1))
    }
    fn nth(&mut self, n: usize) -> Option<C32> {
        let s = self.0.nth(n)?;
        Some(C32::new(s.i as f32 * self.1, s.q as f32 * self.1))
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

/// Skip up to `k` items of `it`; returns how many were skipped.
fn skip(it: &mut impl Iterator<Item = C32>, k: usize) -> usize {
    if k == 0 {
        return 0;
    }
    match it.size_hint() {
        (lo, Some(hi)) if lo == hi => {
            let k = k.min(lo);
            if k > 0 {
                it.nth(k - 1);
            }
            k
        }
        _ => it.by_ref().take(k).count(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic Gaussian pairs (xorshift + Box-Muller).
    struct Noise(u64);
    impl Noise {
        fn next(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        }
        fn gauss(&mut self) -> (f64, f64) {
            let r = (-2.0 * self.next().ln()).sqrt();
            let t = 2.0 * std::f64::consts::PI * self.next();
            (r * t.cos(), r * t.sin())
        }
    }

    fn iq(i: f64, q: f64) -> Iq16 {
        let c = |x: f64| x.round().clamp(-32768.0, 32767.0) as i16;
        Iq16 { i: c(i), q: c(q) }
    }

    fn spec(fs: f64, budget: usize) -> WfSpec {
        WfSpec {
            fs,
            bins: 1024,
            budget,
            fps: 20.0,
            droop: Droop::None,
            fft_per_bin: 2,
        }
    }

    #[test]
    fn rx_channel_input_and_fresh_lines() {
        // a 48 kS/s RX channel: -20 dBFS tone at -5 kHz
        let fs = 48_000.0;
        let mut e = WfEngine::new(spec(fs, 30));
        let a = web888_radio::RX_SINE_FULL_SCALE * 0.1;
        let w = 2.0 * std::f64::consts::PI * -5e3 / fs;
        let x: Vec<Iq32> = (0..4800)
            .map(|k| Iq32 {
                i: (a * (w * k as f64).cos()) as i32,
                q: (a * (w * k as f64).sin()) as i32,
            })
            .collect();
        e.push_rx(x.iter().copied(), true);
        let l = e.line().unwrap();
        assert!(l.fresh);
        let (k, peak) =
            l.db.iter()
                .enumerate()
                .fold((0, f32::MIN), |m, (k, &d)| if d > m.1 { (k, d) } else { m });
        let f = l.first_offset_hz + k as f64 * l.bin_hz;
        assert!((f + 5e3).abs() <= l.bin_hz, "{f}");
        assert!((-23.0..=-19.9).contains(&peak), "{peak}");
        // nothing new: the next line repeats the newest samples
        let l = e.line().unwrap();
        assert!(!l.fresh && l.ffts == 1);
    }

    #[test]
    fn tone_level_and_position() {
        let fs = 1_920_000.0;
        let mut e = WfEngine::new(spec(fs, 100));
        // -20 dBFS complex tone at +300 kHz
        let a = WF_SINE_FULL_SCALE * 0.1;
        let w = 2.0 * std::f64::consts::PI * 300e3 / fs;
        let x: Vec<Iq16> = (0..96_000)
            .map(|k| iq(a * (w * k as f64).cos(), a * (w * k as f64).sin()))
            .collect();
        e.push(&x, true);
        let l = e.line().unwrap();
        assert!(l.ffts > 50, "{}", l.ffts);
        let (k, peak) =
            l.db.iter()
                .enumerate()
                .fold((0, f32::MIN), |m, (k, &d)| if d > m.1 { (k, d) } else { m });
        let f = l.first_offset_hz + k as f64 * l.bin_hz;
        assert!((f - 300e3).abs() <= l.bin_hz, "{f}");
        assert!((-23.0..=-19.9).contains(&peak), "{peak}");
        assert_eq!(l.bin_hz, fs / 1024.0);
        // display bins merge FFT bin pairs: centres half an FFT bin up
        let fft_bin = fs / e.fft_n() as f64;
        assert!((l.first_offset_hz - (-fs / 2.0 + fft_bin / 2.0)).abs() < 1e-6);
    }

    #[test]
    fn narrow_span_lines_reuse_newest_samples() {
        let fs = 15_000.0; // decim 8192
        let mut e = WfEngine::new(spec(fs, 30));
        let x = vec![iq(100.0, 0.0); 750]; // one 20 fps interval
        e.push(&x, true);
        assert!(e.line().is_none(), "fewer than N samples so far");
        for _ in 0..3 {
            e.push(&x, true);
        }
        assert_eq!(e.line().unwrap().ffts, 1);
        e.push(&x, true);
        assert_eq!(
            e.line().unwrap().ffts,
            1,
            "sliding window over the newest N"
        );
    }

    #[test]
    fn budget_is_spread_over_the_interval() {
        let fs = 1_920_000.0;
        let mut e = WfEngine::new(spec(fs, 10));
        let per_line = 96_000;
        // first line: stride from the expected rate (fs / fps / budget)
        let x = vec![iq(0.0, 0.0); per_line];
        e.push(&x, true);
        assert_eq!(e.line().unwrap().ffts, 10);
        // twice the samples: the budget caps the count, the next line's
        // stride widens to cover all of it
        let x = vec![iq(0.0, 0.0); 2 * per_line];
        e.push(&x, true);
        assert_eq!(e.line().unwrap().ffts, 10);
        e.push(&x, true);
        assert_eq!(e.line().unwrap().ffts, 10);
        assert_eq!(e.stride, 2 * per_line / 10);
    }

    #[test]
    fn lines_do_not_depend_on_the_chunking() {
        // budget 4 at 1.92 MS/s, 20 fps: the stride skips most samples
        let mut rng = Noise(5);
        let x: Vec<Iq16> = (0..96_000 * 4)
            .map(|_| {
                let (a, b) = rng.gauss();
                iq(300.0 * a, 300.0 * b)
            })
            .collect();
        let run = |chunk: usize| {
            let mut e = WfEngine::new(spec(1_920_000.0, 4));
            let mut lines = Vec::new();
            for l in x.chunks(96_000) {
                for c in l.chunks(chunk) {
                    e.push(c, true);
                }
                lines.push(e.line().unwrap());
            }
            lines
        };
        let whole = run(96_000);
        assert!(whole.iter().skip(1).all(|l| l.ffts == 4));
        assert_eq!(whole, run(997));
        assert_eq!(whole, run(1));
    }

    #[test]
    fn gaps_are_not_spanned() {
        let fs = 1_920_000.0;
        let mut e = WfEngine::new(spec(fs, 1000));
        let n = e.fft_n();
        let x = vec![iq(1.0, 0.0); n - 1];
        e.push(&x, true);
        e.push(&x, false);
        e.push(&x, false);
        assert!(e.line().is_none());
    }

    /// Spread (max - min) of the per-line mean level (dB) of noise at
    /// fs 1.92 MS/s, 20 fps. `burst` gates it at 100 Hz (1 ms at +30 dB, as
    /// from a switch-mode supply). `jitter` starts each line at a random
    /// point of the burst cycle, like the stock server's one-shot blocks.
    fn line_spread(budget: usize, burst: bool, jitter: bool) -> f64 {
        let fs = 1_920_000.0;
        let per_line = 96_000;
        let mut e = WfEngine::new(spec(fs, budget));
        let mut rng = Noise(0x2545_f491_4f6c_dd1d);
        let sigma = 30.0;
        let mut means = Vec::new();
        let mut k = 0usize;
        for _ in 0..30 {
            if jitter {
                k += (rng.next() * 19_200.0) as usize;
            }
            let x: Vec<Iq16> = (0..per_line)
                .map(|_| {
                    let on = burst && (k % 19_200) < 1_920;
                    k += 1;
                    let s = if on { sigma * 31.6 } else { sigma };
                    let (a, b) = rng.gauss();
                    iq(a * s, b * s)
                })
                .collect();
            e.push(&x, !jitter);
            let l = e.line().unwrap();
            let lin: f64 = l.db.iter().map(|&d| 10f64.powf(d as f64 / 10.0)).sum();
            means.push(10.0 * (lin / l.db.len() as f64).log10());
        }
        let m = &means[2..]; // after the stride settled
        m.iter().cloned().fold(f64::MIN, f64::max) - m.iter().cloned().fold(f64::MAX, f64::min)
    }

    #[test]
    fn impulsive_noise_reads_steady() {
        // design.md §5.2: within 0.3 dB of a stationary-noise run
        let steady = line_spread(1000, false, false);
        let impulsive = line_spread(1000, true, false);
        assert!(impulsive < steady + 0.3, "{impulsive:.2} vs {steady:.2} dB");
        // one short snapshot per line (the stock behaviour) does not
        let snapshot = line_spread(1, true, true);
        assert!(snapshot > 3.0, "{snapshot:.2} dB");
    }

    #[test]
    fn droop_correction_inverts_the_cic() {
        let fs = 1_920_000.0;
        let decim = 64;
        let mut s = spec(fs, 1000);
        s.droop = Droop::Wf {
            adc_hz: web888_radio::ADC_HZ_HF,
            decim,
        };
        let e = WfEngine::new(s);
        let n = e.fft_n();
        for k in [n / 8, n / 4, n / 2, n - n / 4] {
            let f = if k < n / 2 {
                k as f64
            } else {
                k as f64 - n as f64
            } * fs
                / n as f64;
            let m = web888_radio::wf_chain_mag(f, web888_radio::ADC_HZ_HF, decim);
            let want = (1.0 / (m * m)).min(100.0);
            let got = (e.scale[k] / e.scale[0]) as f64;
            assert!((got / want - 1.0).abs() < 1e-3, "bin {k}: {got} vs {want}");
        }
    }
}
