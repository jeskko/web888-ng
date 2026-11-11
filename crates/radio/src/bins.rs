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

    /// The zoom engine's limits; none for a source without one.
    fn zoom_caps(&self) -> Option<ZoomCaps> {
        None
    }

    /// Queue `jobs` in order; returns how many the queue took (the rest
    /// wait for room).
    fn submit_zoom(&mut self, _jobs: &[ZoomJob]) -> Result<usize> {
        Err(crate::Error::Unsupported("no zoom engine"))
    }

    /// Append the jobs' results that came since the last call; returns
    /// how many.
    fn read_zoom(&mut self, _out: &mut Vec<ZoomResult>) -> Result<usize> {
        Ok(0)
    }
}

/// The zoom engine (`hw/docs/channelizer.md` §3.9): jobs of a few bins'
/// FFTs, captured from the frames in a work RAM and returned as power
/// spectra.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoomCaps {
    /// Jobs the queue holds.
    pub queue: usize,
    /// Work RAM samples, in `contexts` equal parts: a job of up to one
    /// part's samples takes one, a larger one all of them.
    pub work_samples: usize,
    pub contexts: usize,
    /// Most bins a job.
    pub max_bins: usize,
    /// FFT lengths 2^logn.
    pub min_logn: u8,
    pub max_logn: u8,
    /// Tags (jobs outstanding at most).
    pub tags: usize,
}

impl ZoomCaps {
    /// The bitstream's engine (and the iq backend's model of it).
    pub const ENGINE: ZoomCaps = ZoomCaps {
        queue: 16,
        work_samples: 4096,
        contexts: 2,
        max_bins: 256,
        min_logn: 3,
        max_logn: 11,
        tags: 64,
    };

    /// Samples a context holds.
    pub fn context_samples(&self) -> usize {
        self.work_samples / self.contexts
    }

    /// Contexts a job of `samples` takes (none if it doesn't fit).
    pub fn contexts_for(&self, samples: usize) -> Option<usize> {
        if samples <= self.context_samples() {
            Some(1)
        } else if samples <= self.work_samples {
            Some(self.contexts)
        } else {
            None
        }
    }

    /// Whether the engine takes `job`.
    pub fn takes(&self, job: &ZoomJob) -> bool {
        (job.tag as usize) < self.tags
            && job.shift <= 7
            && (self.min_logn..=self.max_logn).contains(&job.logn)
            && (1..=self.max_bins).contains(&(job.bins as usize))
            && job.samples() <= self.work_samples
    }
}

/// A job: bins `k0 .. k0 + bins` (bin numbers wrap at the source's bin
/// count, so on a complex input a job may cross bin 0), an FFT of
/// n = 2^logn frames each, its samples the 25-bit bins shifted right by
/// `shift` into the work RAM's 22 bits (saturating).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZoomJob {
    /// 0..64, the result's.
    pub tag: u8,
    /// 0..=7.
    pub shift: u8,
    /// 3..=11.
    pub logn: u8,
    /// 1..=256, bins · n ≤ 4096.
    pub bins: u16,
    pub k0: u16,
}

impl ZoomJob {
    pub fn n(&self) -> usize {
        1 << self.logn
    }

    /// Work RAM samples: bins · n.
    pub fn samples(&self) -> usize {
        self.bins as usize * self.n()
    }

    /// Points in the result: n/2 a bin.
    pub fn points(&self) -> usize {
        self.samples() / 2
    }

    /// The job's word in the bitstream's queue:
    /// `{tag[5:0], shift[2:0], logn[3:0], bins - 1 [7:0], k0[10:0]}`.
    pub fn word(&self) -> u32 {
        (self.tag as u32 & 0x3f) << 26
            | (self.shift as u32 & 7) << 23
            | (self.logn as u32 & 0xf) << 19
            | ((self.bins as u32).wrapping_sub(1) & 0xff) << 11
            | (self.k0 as u32 & 0x7ff)
    }
}

/// A job's power spectrum. For each of its bins, lowest first, the FFT
/// points j = -n/4 .. n/4 - 1 (the middle 30 kHz of the bin, ascending
/// frequency): |X|² of the n-point FFT of the bin's samples, one block of
/// n frames, Blackman-Harris windowed (4 terms, periodic, peak 1), the
/// samples scaled so a full-scale sine has |z| = 1. A full-scale tone on
/// a point's frequency reads (Σw)² = (0.35875 n)² there; dividing by
/// (Σw)² gives the waterfall's levels (`web888_dsp::wf`).
#[derive(Clone, Debug, PartialEq)]
pub struct ZoomResult {
    pub tag: u8,
    /// The capture's first frame, its low 16 bits.
    pub frame: u16,
    /// A sample saturated: the job wants a larger shift.
    pub saturated: bool,
    /// bins · n/2 points.
    pub power: Vec<f32>,
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
