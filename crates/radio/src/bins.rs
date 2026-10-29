// SPDX-License-Identifier: GPL-3.0-or-later
//! Bin sources (`doc/m6-channelizer.md` §2): a polyphase channelizer
//! splits the band into bins 30 kHz apart, each a complex stream at 60
//! kS/s; the source sends the bins selected, and every so many frames the
//! power of all of them. The channelizer bitstream does this in the FPGA
//! (`hw/docs/channelizer.md` §3.7), the iq backend in software.

use crate::{Iq32, Result};
use std::sync::Arc;

/// What a bin source offers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BinCaps {
    /// Bin spacing (Hz).
    pub bin_hz: f64,
    /// Frames per second: each bin's sample rate.
    pub frame_rate: f64,
    /// Bins 0..bins (the ones that can be selected).
    pub bins: usize,
    /// Frequency of bin 0 (Hz).
    pub first_hz: f64,
    /// Complex input (iq backend): bins from `bins / 2` on are below
    /// bin 0, `(k - bins) · bin_hz` from it. Real input (the ADC): every
    /// bin is above.
    pub wraps: bool,
    /// Most bins a frame can carry.
    pub max_selected: usize,
}

impl BinCaps {
    /// Centre frequency of bin `k`.
    pub fn freq(&self, k: usize) -> f64 {
        let k = if self.wraps && k >= self.bins / 2 {
            k as f64 - self.bins as f64
        } else {
            k as f64
        };
        self.first_hz + k * self.bin_hz
    }

    /// The bin nearest `hz` and the offset of `hz` from its centre; none
    /// outside the bins.
    pub fn bin_of(&self, hz: f64) -> Option<(usize, f64)> {
        let k = ((hz - self.first_hz) / self.bin_hz).round();
        let n = self.bins as f64;
        let (lo, hi) = if self.wraps {
            (-(n / 2.0), n / 2.0 - 1.0)
        } else {
            (0.0, n - 1.0)
        };
        if !(lo..=hi).contains(&k) {
            return None;
        }
        let b = if k < 0.0 {
            (k + n) as usize
        } else {
            k as usize
        };
        Some((b, hz - self.freq(b)))
    }
}

/// Consecutive frames with one selection: frame `first + k` carries bin
/// `bins[i]`'s sample at `iq[k * bins.len() + i]`. One allocation per run
/// of frames, not per frame (60k frames a second).
#[derive(Clone, Debug, PartialEq)]
pub struct BinFrames {
    /// The first frame's number (frames since the source started).
    pub first: u64,
    /// The selection they used (`BinRadio::set_bins`' result).
    pub generation: u8,
    /// The selected bins, ascending; samples scaled like RX samples (a
    /// full-scale sine reads `RX_SINE_FULL_SCALE`).
    pub bins: Arc<[usize]>,
    /// Frames in the run (also when no bins are selected).
    pub count: usize,
    pub iq: Vec<Iq32>,
}

impl BinFrames {
    pub fn new(first: u64, generation: u8, bins: Arc<[usize]>) -> Self {
        BinFrames {
            first,
            generation,
            bins,
            count: 0,
            iq: Vec::new(),
        }
    }

    /// Frame `first + k`'s samples.
    pub fn frame(&self, k: usize) -> &[Iq32] {
        let n = self.bins.len();
        &self.iq[k * n..(k + 1) * n]
    }

    /// The number after the last frame.
    pub fn end(&self) -> u64 {
        self.first + self.count as u64
    }

    /// Frame `frame` with `generation`'s selection `bins` would continue
    /// this run.
    pub fn continued_by(&self, frame: u64, generation: u8, bins: &Arc<[usize]>) -> bool {
        self.end() == frame && self.generation == generation && Arc::ptr_eq(&self.bins, bins)
    }

    /// Append frame `frame` (its samples, `bins.len()` of them) to the last
    /// run in `out`, or start a new one.
    pub fn append(
        out: &mut Vec<BinFrames>,
        frame: u64,
        generation: u8,
        bins: &Arc<[usize]>,
        iq: impl IntoIterator<Item = Iq32>,
    ) {
        let cont = out
            .last()
            .is_some_and(|b| b.continued_by(frame, generation, bins));
        if !cont {
            out.push(BinFrames::new(frame, generation, bins.clone()));
        }
        let b = out.last_mut().expect("pushed");
        b.iq.extend(iq);
        b.count += 1;
    }
}

/// One frame on its own (tests and tools; the sources hand runs).
#[derive(Clone, Debug, PartialEq)]
pub struct SplitFrame {
    pub frame: u64,
    pub generation: u8,
    pub bins: Arc<[usize]>,
    pub iq: Vec<Iq32>,
}

/// The runs' frames one by one.
pub fn split(runs: &[BinFrames]) -> Vec<SplitFrame> {
    runs.iter()
        .flat_map(|r| {
            (0..r.count).map(move |k| SplitFrame {
                frame: r.first + k as u64,
                generation: r.generation,
                bins: r.bins.clone(),
                iq: r.frame(k).to_vec(),
            })
        })
        .collect()
}

/// A channelizer.
pub trait BinRadio: Send {
    fn bin_caps(&self) -> BinCaps;

    /// Select `bins` (at most `max_selected`, the lowest kept if more;
    /// unsorted and repeated ones are fine). Takes effect at a frame
    /// boundary; returns the generation the frames using it carry.
    fn set_bins(&mut self, bins: &[usize]) -> Result<u8>;

    /// Append the frames that came since the last call (runs continuing
    /// the last one in `out` are merged into it); returns how many frames.
    fn read_bins(&mut self, out: &mut Vec<BinFrames>) -> Result<usize>;

    /// Power lines of `frames` frames each (0: none).
    fn set_power(&mut self, frames: u16) -> Result<()>;

    /// The next power line, if one is complete: every bin's mean |X|²
    /// over the line, a full-scale sine reading 1. Returns its number.
    fn read_power(&mut self, line: &mut Vec<f32>) -> Result<Option<u32>>;
}

/// The selection as a source keeps it: sorted, without repeats, at most
/// `max` (the lowest), every bin below `bins`.
pub fn selection(bins: &[usize], nbins: usize, max: usize) -> Vec<usize> {
    let mut v: Vec<usize> = bins.iter().copied().filter(|&b| b < nbins).collect();
    v.sort_unstable();
    v.dedup();
    v.truncate(max);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bins_and_frequencies() {
        let real = BinCaps {
            bin_hz: 30e3,
            frame_rate: 60e3,
            bins: 2048,
            first_hz: 0.0,
            wraps: false,
            max_selected: 64,
        };
        assert_eq!(real.freq(236), 7.08e6);
        assert_eq!(real.bin_of(7.074e6), Some((236, -6000.0)));
        assert_eq!(real.bin_of(-20e3), None);
        let iq = BinCaps {
            first_hz: 8.6e6,
            bins: 256,
            wraps: true,
            ..real
        };
        assert_eq!(iq.freq(1), 8.63e6);
        assert_eq!(iq.freq(255), 8.57e6);
        assert_eq!(iq.freq(128), 8.6e6 - 128.0 * 30e3);
        assert_eq!(iq.bin_of(7.074e6), Some((205, 4000.0)));
        assert_eq!(iq.bin_of(12.5e6), None);
        assert_eq!(selection(&[9, 3, 3, 300, 1], 256, 2), vec![1, 3]);
    }
}
