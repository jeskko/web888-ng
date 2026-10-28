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

/// One frame: a sample of each selected bin.
#[derive(Clone, Debug, PartialEq)]
pub struct BinFrame {
    /// Frames since the source started.
    pub frame: u64,
    /// The selection it used (`BinRadio::set_bins`' result).
    pub generation: u8,
    /// The selected bins, ascending; `iq[i]` is bin `bins[i]`'s sample,
    /// scaled like RX samples (a full-scale sine reads
    /// `RX_SINE_FULL_SCALE`).
    pub bins: Arc<[usize]>,
    pub iq: Vec<Iq32>,
}

/// A channelizer.
pub trait BinRadio: Send {
    fn bin_caps(&self) -> BinCaps;

    /// Select `bins` (at most `max_selected`, the lowest kept if more;
    /// unsorted and repeated ones are fine). Takes effect at a frame
    /// boundary; returns the generation the frames using it carry.
    fn set_bins(&mut self, bins: &[usize]) -> Result<u8>;

    /// Append the frames that came since the last call; returns how many.
    fn read_bins(&mut self, out: &mut Vec<BinFrame>) -> Result<usize>;

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
