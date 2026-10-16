// SPDX-License-Identifier: GPL-3.0-or-later
//! Channel filter design with the hardware CIC droop folded in.
//!
//! The passband `[lo, hi]` (Hz, relative to the hardware NCO, may be
//! asymmetric: USB 300..2700, LSB -2700..-300, AM -5000..5000) is designed
//! by frequency sampling: the desired response is `1 / H_cic(f)` inside the
//! band (so the cascade hardware + filter is flat), 0 outside, with the
//! -6 dB edges in the middle of the transition bands. The result is then
//! windowed in time with a Kaiser window for the requested stopband.

use crate::C32;
use crate::fir::{kaiser_beta, kaiser_len, kaiser_window};
use num_complex::Complex64;
use rustfft::FftPlanner;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChannelSpec {
    pub lo_hz: f64,
    pub hi_hz: f64,
    /// Width of each transition band.
    pub transition_hz: f64,
    pub atten_db: f64,
}

/// Hardware response to compensate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Droop {
    None,
    /// Web-888 RX chain (CIC1 N=3 R=256, CIC2 N=5 R=r2) at ADC rate `adc_hz`.
    Rx {
        adc_hz: f64,
        r2: u16,
    },
}

impl Droop {
    pub fn mag(&self, f: f64) -> f64 {
        match *self {
            Droop::None => 1.0,
            Droop::Rx { adc_hz, r2 } => web888_radio::rx_chain_mag(f, adc_hz, r2),
        }
    }
}

/// Complex FIR taps for `spec` at sample rate `fs` (output order: h[0] is
/// applied to the newest sample).
pub fn design(spec: ChannelSpec, fs: f64, droop: Droop) -> Vec<C32> {
    assert!(spec.lo_hz < spec.hi_hz, "lo must be below hi");
    assert!(spec.transition_hz > 0.0);
    let n = kaiser_len(spec.atten_db, spec.transition_hz / fs);
    let m = (n - 1) / 2;
    let l = (8 * n).next_power_of_two().max(4096);

    // desired response on an L-point grid
    let edge_lo = spec.lo_hz - spec.transition_hz / 2.0;
    let edge_hi = spec.hi_hz + spec.transition_hz / 2.0;
    let mut grid: Vec<Complex64> = (0..l)
        .map(|k| {
            let kk = if k < l / 2 {
                k as f64
            } else {
                k as f64 - l as f64
            };
            let f = kk / l as f64 * fs;
            if f >= edge_lo && f <= edge_hi {
                // cap the boost so a band touching the CIC nulls stays sane
                Complex64::new(1.0 / droop.mag(f).max(0.05), 0.0)
            } else {
                Complex64::new(0.0, 0.0)
            }
        })
        .collect();
    FftPlanner::new().plan_fft_inverse(l).process(&mut grid);

    // centre, window, scale (rustfft is unnormalised)
    let w = kaiser_window(n, kaiser_beta(spec.atten_db));
    let mut h: Vec<Complex64> = (0..n)
        .map(|i| {
            let t = i as isize - m as isize;
            grid[t.rem_euclid(l as isize) as usize] * (w[i] / l as f64)
        })
        .collect();

    // exact gain at the band centre: |H_filter(fc)| * H_cic(fc) = 1
    let fc = (spec.lo_hz + spec.hi_hz) / 2.0;
    let g = response(&h, fc, fs) * droop.mag(fc);
    h.iter_mut().for_each(|x| *x /= g);
    h.iter()
        .map(|c| C32::new(c.re as f32, c.im as f32))
        .collect()
}

/// |H(f)| of complex taps (linear-phase centred taps).
pub fn response<T: Copy + Into<Complex64>>(h: &[T], f: f64, fs: f64) -> f64 {
    let w = -2.0 * std::f64::consts::PI * f / fs;
    h.iter()
        .enumerate()
        .map(|(k, &c)| c.into() * Complex64::from_polar(1.0, w * k as f64))
        .sum::<Complex64>()
        .norm()
}

/// Standard channel shapes at hardware rate `fs`.
impl ChannelSpec {
    pub fn usb(lo: f64, hi: f64) -> Self {
        ChannelSpec {
            lo_hz: lo,
            hi_hz: hi,
            transition_hz: 200.0,
            atten_db: 80.0,
        }
    }
    pub fn lsb(lo: f64, hi: f64) -> Self {
        ChannelSpec {
            lo_hz: -hi,
            hi_hz: -lo,
            transition_hz: 200.0,
            atten_db: 80.0,
        }
    }
    pub fn am(half_bw: f64) -> Self {
        ChannelSpec {
            lo_hz: -half_bw,
            hi_hz: half_bw,
            transition_hz: 500.0,
            atten_db: 80.0,
        }
    }
}
