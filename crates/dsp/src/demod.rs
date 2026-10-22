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
