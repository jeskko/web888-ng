// SPDX-License-Identifier: GPL-3.0-or-later
//! The iq backend's channelizer (`doc/m6-channelizer.md` §2): the complex
//! input through `PfbComplex` with M = rate / 30 kHz branches, so its bins
//! are 30 kHz apart at 60 kS/s, as the channelizer bitstream's are. Bin k
//! is `center + k · 30 kHz` (k ≥ M/2: below the centre). Selection and
//! power lines behave as the bitstream's (`hw/docs/channelizer.md` §3.7):
//! a new selection starts at a frame, under a new generation; a power line
//! is the mean |X|² of every bin over N frames.

use std::collections::VecDeque;
use std::sync::Arc;
use web888_dsp::C32;
use web888_dsp::pfb::{PfbComplex, PfbSpec};
use web888_radio::bins::{BinCaps, BinFrame, selection};
use web888_radio::{Iq32, RX_SINE_FULL_SCALE};

/// Bin spacing, as the bitstream's.
pub const BIN_HZ: f64 = 30e3;
/// Most bins a frame, as the bitstream's.
pub const MAX_SELECTED: usize = 64;
/// Frames kept for a reader that falls behind (about 100 ms).
const QUEUE: usize = 6000;

pub struct IqBins {
    caps: BinCaps,
    pfb: PfbComplex,
    frame: u64,
    /// The selection in use and its generation; the next one, if set.
    sel: Arc<[usize]>,
    generation: u8,
    next: Option<(Arc<[usize]>, u8)>,
    frames: VecDeque<BinFrame>,
    dropped: u64,
    /// Power: frames per line (0: off), the sums, frames in, lines done.
    per_line: u16,
    sum: Vec<f64>,
    in_line: u16,
    line: u32,
    lines: VecDeque<(u32, Vec<f32>)>,
}

impl IqBins {
    /// For input at `rate_hz` (a multiple of 30 kHz: 2 · 30 kHz · 2^k)
    /// around `center_hz`.
    pub fn new(center_hz: f64, rate_hz: f64) -> Self {
        let m = (rate_hz / BIN_HZ).round() as usize;
        assert!(
            m.is_power_of_two() && (m as f64 * BIN_HZ - rate_hz).abs() < 1e-6,
            "rate {rate_hz} is not 30 kHz x a power of two"
        );
        let spec = PfbSpec {
            fs: rate_hz,
            branches: m,
            ..PfbSpec::web888()
        };
        IqBins {
            caps: BinCaps {
                bin_hz: BIN_HZ,
                frame_rate: spec.out_rate(),
                bins: m,
                first_hz: center_hz,
                wraps: true,
                max_selected: MAX_SELECTED,
            },
            pfb: PfbComplex::new(spec),
            frame: 0,
            sel: Arc::from(Vec::new()),
            generation: 0,
            next: None,
            frames: VecDeque::new(),
            dropped: 0,
            per_line: 0,
            sum: vec![0.0; m],
            in_line: 0,
            line: 0,
            lines: VecDeque::new(),
        }
    }

    pub fn caps(&self) -> BinCaps {
        self.caps
    }

    /// Select `bins` from the next frame on; returns its generation.
    pub fn set_bins(&mut self, bins: &[usize]) -> u8 {
        let generation = self
            .next
            .as_ref()
            .map_or(self.generation, |n| n.1)
            .wrapping_add(1);
        let s = selection(bins, self.caps.bins, MAX_SELECTED);
        self.next = Some((Arc::from(s), generation));
        generation
    }

    /// Power lines of `frames` frames (0: off), from the next frame.
    pub fn set_power(&mut self, frames: u16) {
        self.per_line = frames;
        self.in_line = 0;
        self.sum.iter_mut().for_each(|s| *s = 0.0);
    }

    /// Frames made so far (selected bins or not).
    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// Frames the reader lost by falling behind.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Input samples (a complex sine of amplitude 1 is full scale).
    pub fn push(&mut self, x: &[C32]) {
        let IqBins {
            pfb,
            frame,
            sel,
            generation,
            next,
            frames,
            dropped,
            per_line,
            sum,
            in_line,
            line,
            lines,
            ..
        } = self;
        pfb.push(x, |u| {
            if let Some((s, g)) = next.take() {
                *sel = s;
                *generation = g;
            }
            // a frame without bins too, as the hardware sends: its
            // reader's clock (RX frames from bins keep coming)
            let k = RX_SINE_FULL_SCALE as f32;
            let iq = sel
                .iter()
                .map(|&b| {
                    let z = u[b] * k;
                    Iq32 {
                        i: z.re.round().clamp(i32::MIN as f32, i32::MAX as f32) as i32,
                        q: z.im.round().clamp(i32::MIN as f32, i32::MAX as f32) as i32,
                    }
                })
                .collect();
            if frames.len() >= QUEUE {
                frames.pop_front();
                *dropped += 1;
            }
            frames.push_back(BinFrame {
                frame: *frame,
                generation: *generation,
                bins: sel.clone(),
                iq,
            });
            if *per_line > 0 {
                for (s, z) in sum.iter_mut().zip(u) {
                    *s += z.norm_sqr() as f64;
                }
                *in_line += 1;
                if *in_line == *per_line {
                    let n = *per_line as f64;
                    let l: Vec<f32> = sum.iter().map(|&s| (s / n) as f32).collect();
                    if lines.len() >= 16 {
                        lines.pop_front();
                    }
                    lines.push_back((*line, l));
                    *line = line.wrapping_add(1);
                    *in_line = 0;
                    sum.iter_mut().for_each(|s| *s = 0.0);
                }
            }
            *frame += 1;
        });
    }

    /// Move the frames since the last call to `out`; returns how many.
    pub fn read_bins(&mut self, out: &mut Vec<BinFrame>) -> usize {
        let n = self.frames.len();
        out.extend(self.frames.drain(..));
        n
    }

    /// The oldest power line not read yet, into `line`.
    pub fn read_power(&mut self, line: &mut Vec<f32>) -> Option<u32> {
        let (n, l) = self.lines.pop_front()?;
        *line = l;
        Some(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 7.68e6;
    const CENTER: f64 = 8.6e6;

    fn tone(f: f64, a: f32, n: usize, t0: usize) -> Vec<C32> {
        (t0..t0 + n)
            // the phase wrapped in f64 (f32 of thousands of radians is noisy)
            .map(|i| {
                let ph = (std::f64::consts::TAU * (f - CENTER) / RATE * i as f64)
                    .rem_euclid(std::f64::consts::TAU);
                C32::from_polar(a, ph as f32)
            })
            .collect()
    }

    #[test]
    fn a_tone_lands_in_its_bin_at_its_level() {
        let mut b = IqBins::new(CENTER, RATE);
        let c = b.caps();
        assert_eq!((c.bins, c.frame_rate), (256, 60e3));
        // 7.074 MHz at -20 dBFS: bin 205, 4 kHz above its centre
        let (k, off) = c.bin_of(7.074e6).unwrap();
        assert_eq!((k, off), (205, 4000.0));
        let generation = b.set_bins(&[k, k + 1, 3]);
        b.push(&tone(7.074e6, 0.1, 128 * 200, 0));
        let mut f = vec![];
        let n = b.read_bins(&mut f);
        assert!(n > 100, "{n} frames");
        let last = f.last().unwrap();
        assert_eq!(
            (last.generation, &*last.bins),
            (generation, &[3, 205, 206][..])
        );
        let amp =
            |s: Iq32| ((s.i as f64).powi(2) + (s.q as f64).powi(2)).sqrt() / RX_SINE_FULL_SCALE;
        // in its bin at 0.1 (flat to +-21 kHz), nothing in the far bin
        let a = amp(last.iq[1]);
        assert!((a / 0.1 - 1.0).abs() < 0.01, "{a}");
        assert!(amp(last.iq[0]) < 1e-5);
        // the samples turn at the offset: 4 kHz at 60 kS/s
        let ph = |s: Iq32| (s.q as f64).atan2(s.i as f64);
        let d = (ph(f[n - 1].iq[1]) - ph(f[n - 2].iq[1])).rem_euclid(std::f64::consts::TAU);
        assert!(
            (d - std::f64::consts::TAU * 4000.0 / 60e3).abs() < 1e-3,
            "{d}"
        );
    }

    #[test]
    fn frames_come_without_bins_selected() {
        // BinRx's RX frames follow the bin frames: none selected must not
        // stop them
        let mut b = IqBins::new(CENTER, RATE);
        // a frame per 128 samples, after the bank's filter has filled
        b.push(&tone(7.074e6, 0.1, 128 * 200, 0));
        let mut out = Vec::new();
        assert!(b.read_bins(&mut out) > 150);
        assert!(out.iter().all(|f| f.iq.is_empty()));
    }

    #[test]
    fn a_new_selection_starts_at_a_frame() {
        let mut b = IqBins::new(CENTER, RATE);
        let g1 = b.set_bins(&[10, 11]);
        let x = tone(9.0e6, 0.5, 128 * 120, 0);
        b.push(&x[..128 * 100]);
        let g2 = b.set_bins(&[12; 100]);
        // a selection set twice before a frame: the second wins
        let g3 = b.set_bins(&(0..100).collect::<Vec<_>>());
        assert_eq!((g2, g3), (g1 + 1, g1 + 2));
        b.push(&x[128 * 100..]);
        let mut f = vec![];
        b.read_bins(&mut f);
        let (old, new): (Vec<_>, Vec<_>) = f.iter().partition(|f| f.generation == g1);
        assert!(!old.is_empty() && !new.is_empty());
        assert!(old.iter().all(|f| f.iq.len() == 2));
        assert!(
            new.iter()
                .all(|f| f.generation == g3 && f.iq.len() == MAX_SELECTED && f.bins[63] == 63)
        );
        // frames counted in order, the switch between two of them
        assert!(
            f.windows(2)
                .all(|w| w[1].frame == w[0].frame + 1 && w[1].generation >= w[0].generation)
        );
    }

    #[test]
    fn the_power_line_is_the_bins_mean_power() {
        let mut b = IqBins::new(CENTER, RATE);
        b.set_power(30);
        b.set_bins(&[205]);
        let x = tone(7.074e6, 0.1, 128 * 200, 0);
        b.push(&x);
        let mut line = vec![];
        let n = b.read_power(&mut line).unwrap();
        assert_eq!((n, line.len()), (0, 256));
        // -20 dBFS in bin 205, the rest far below
        assert!(
            (10.0 * line[205].log10() + 20.0).abs() < 0.1,
            "{}",
            line[205]
        );
        assert!(line[3] < 1e-10, "{}", line[3]);
        assert!(
            b.read_power(&mut line).is_some(),
            "several lines in 200 frames"
        );
    }
}
