// SPDX-License-Identifier: GPL-3.0-or-later
//! DSP building blocks for web888d. Samples are `Complex32` (f32 I/Q);
//! design-time math is f64.

pub mod agc;
pub mod audio;
pub mod binrx;
pub mod chan;
pub mod decim;
pub mod demod;
pub mod fir;
pub mod nb;
pub mod nco;
pub mod nr;
pub mod ols;
pub mod pfb;
pub mod pfbfx;
pub mod squelch;
pub mod wf;

pub use num_complex::Complex32 as C32;

/// Convert hardware RX samples to f32, scaled so that a full-scale ADC sine
/// has |z| = 1 (before any CIC droop).
pub fn rx_to_c32(s: web888_radio::Iq32) -> C32 {
    const K: f32 = (1.0 / web888_radio::RX_SINE_FULL_SCALE) as f32;
    C32::new(s.i as f32 * K, s.q as f32 * K)
}
