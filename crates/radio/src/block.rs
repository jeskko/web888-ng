// SPDX-License-Identifier: GPL-3.0-or-later
//! Sample blocks as the hw thread hands them to its consumers (HPSDR, web
//! audio, waterfall). Shared read-only through `Arc`, so one read serves
//! every consumer without copies.

use crate::{Iq16, Iq32};

/// A run of whole RX frames (`nch` samples each, channel 0 first).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RxBlock {
    /// Index of the first frame since the RX stream started. A consumer
    /// sees lost blocks as a jump here.
    pub first_frame: u64,
    pub nch: usize,
    pub iq: Vec<Iq32>,
    /// ADC overload seen while these frames were read.
    pub overload: bool,
}

impl RxBlock {
    pub fn frames(&self) -> usize {
        self.iq.len().checked_div(self.nch).unwrap_or(0)
    }

    /// The samples of one channel.
    pub fn channel(&self, ch: usize) -> impl Iterator<Item = Iq32> + '_ {
        self.iq.iter().skip(ch).step_by(self.nch.max(1)).copied()
    }

    /// Frame index just after this block.
    pub fn end_frame(&self) -> u64 {
        self.first_frame + self.frames() as u64
    }
}

/// Samples from one continuous waterfall slot.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WfBlock {
    pub slot: usize,
    /// Set by whoever started the slot; samples of an older generation
    /// belong to a previous tuning and are stale.
    pub generation: u64,
    /// Counts blocks of this slot; a jump means blocks were lost.
    pub seq: u64,
    /// The stream has a discontinuity before these samples (driver block
    /// restart, overrun, retune).
    pub gap_before: bool,
    pub iq: Vec<Iq16>,
}

impl WfBlock {
    /// True if this block directly continues `prev` (same generation, next
    /// in sequence, no gap).
    pub fn continues(&self, prev: &WfBlock) -> bool {
        self.generation == prev.generation && self.seq == prev.seq + 1 && !self.gap_before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_picks_every_nth_sample() {
        let iq = (0..12).map(|k| Iq32 { i: k, q: -k }).collect::<Vec<_>>();
        let b = RxBlock {
            first_frame: 10,
            nch: 3,
            iq,
            overload: false,
        };
        assert_eq!(b.frames(), 4);
        assert_eq!(b.end_frame(), 14);
        let c1: Vec<i32> = b.channel(1).map(|s| s.i).collect();
        assert_eq!(c1, [1, 4, 7, 10]);
    }

    #[test]
    fn continuity() {
        let a = WfBlock {
            generation: 2,
            seq: 5,
            ..Default::default()
        };
        let mut b = WfBlock {
            generation: 2,
            seq: 6,
            ..Default::default()
        };
        assert!(b.continues(&a));
        b.gap_before = true;
        assert!(!b.continues(&a));
        b.gap_before = false;
        b.seq = 7;
        assert!(!b.continues(&a));
        b.seq = 6;
        b.generation = 3;
        assert!(!b.continues(&a));
    }
}
