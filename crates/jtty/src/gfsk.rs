// SPDX-License-Identifier: GPL-3.0-or-later
//! A frame's GFSK baseband trajectory, for subtraction and the timing
//! estimate (jtty-cuf `receiver_dsp.cpp`).

use crate::protocol::gaussian_pulse;
use num_complex::Complex32;
use std::sync::OnceLock;

const PULSE_POINTS: usize = 8193;

/// The pulse on [-1.5, 1.5] symbols, linearly interpolated (error < 1e-6).
fn pulse(t: f64) -> f64 {
    static TABLE: OnceLock<Vec<f64>> = OnceLock::new();
    let v = TABLE.get_or_init(|| {
        (0..PULSE_POINTS)
            .map(|i| gaussian_pulse(-1.5 + 3.0 * i as f64 / (PULSE_POINTS - 1) as f64))
            .collect()
    });
    let x = (t + 1.5) / 3.0 * (PULSE_POINTS - 1) as f64;
    if !(x > 0.0) {
        return v[0];
    }
    let i = x as usize;
    if i + 1 >= v.len() {
        return v[v.len() - 1];
    }
    let f = x - i as f64;
    v[i] + f * (v[i + 1] - v[i])
}

/// Instantaneous frequency in tone units at frame time `u` (symbols).
fn shaped_tone(tones: &[u8], u: f64) -> f64 {
    let symbol = u.floor() as i64;
    let local = u - symbol as f64 - 0.5;
    let n = tones.len() as i64;
    let tone_at = |i: i64| -> f64 {
        if i < 0 {
            tones[0] as f64
        } else if i >= n {
            tones[tones.len() - 1] as f64
        } else {
            tones[i as usize] as f64
        }
    };
    tone_at(symbol - 1) * pulse(local + 1.0)
        + tone_at(symbol) * pulse(local)
        + tone_at(symbol + 1) * pulse(local - 1.0)
}

/// Continuous-phase GFSK (BT = 2, h = 1), unit magnitude, tone 0 at 0 Hz,
/// phase 0 at the frame start. `out[i]` is at frame position
/// `first_sample + i - delay` samples (fractional `delay`: the frame
/// start); outside the frame zero; `ramp` samples of raised-cosine edges.
pub(crate) fn gfsk_baseband(
    tones: &[u8],
    samples_per_symbol: f64,
    delay: f64,
    first_sample: isize,
    count: usize,
    ramp: f64,
) -> Vec<Complex32> {
    let mut out = vec![Complex32::new(0.0, 0.0); count];
    if tones.is_empty() {
        return out;
    }
    let length = tones.len() as f64 * samples_per_symbol;
    let two_pi = 2.0 * std::f64::consts::PI;
    let mut phase = 0.0f64;
    let mut previous = 0.0f64;
    for (i, o) in out.iter_mut().enumerate() {
        let p = first_sample as f64 + i as f64 - delay;
        if p < 0.0 || p >= length {
            continue;
        }
        // midpoint rule for the phase integral
        let step = p - previous;
        let middle = (previous + p) * 0.5;
        phase +=
            two_pi * shaped_tone(tones, middle / samples_per_symbol) * step / samples_per_symbol;
        previous = p;
        let mut envelope = 1.0;
        if ramp > 0.0 {
            if p < ramp {
                envelope = 0.5 * (1.0 - (std::f64::consts::PI * p / ramp).cos());
            }
            let tail = length - p;
            if tail < ramp {
                envelope = 0.5 * (1.0 - (std::f64::consts::PI * tail / ramp).cos());
            }
        }
        *o = Complex32::new(
            (envelope * phase.cos()) as f32,
            (envelope * phase.sin()) as f32,
        );
    }
    out
}
