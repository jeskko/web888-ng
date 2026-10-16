// SPDX-License-Identifier: GPL-3.0-or-later
//! Kaiser-window FIR design.

use std::f64::consts::PI;

/// Zeroth-order modified Bessel function of the first kind.
pub fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..200 {
        term *= q / (k * k) as f64;
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// Kaiser beta for a stopband attenuation in dB.
pub fn kaiser_beta(atten_db: f64) -> f64 {
    if atten_db > 50.0 {
        0.1102 * (atten_db - 8.7)
    } else if atten_db >= 21.0 {
        0.5842 * (atten_db - 21.0).powf(0.4) + 0.07886 * (atten_db - 21.0)
    } else {
        0.0
    }
}

/// Number of taps for attenuation `atten_db` and transition width `tw`
/// (cycles/sample), rounded up to odd.
pub fn kaiser_len(atten_db: f64, tw: f64) -> usize {
    let n = ((atten_db - 7.95) / (14.36 * tw)).ceil() as usize + 1;
    n | 1
}

/// Kaiser window of length `n`.
pub fn kaiser_window(n: usize, beta: f64) -> Vec<f64> {
    let m = (n - 1) as f64;
    let d = bessel_i0(beta);
    (0..n)
        .map(|i| {
            let r = 2.0 * i as f64 / m - 1.0;
            bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / d
        })
        .collect()
}

/// Real lowpass, cutoff `fc` (cycles/sample, -6 dB point), unity DC gain.
pub fn lowpass(n: usize, fc: f64, atten_db: f64) -> Vec<f64> {
    let w = kaiser_window(n, kaiser_beta(atten_db));
    let m = (n - 1) as f64 / 2.0;
    let mut h: Vec<f64> = (0..n)
        .map(|i| {
            let t = i as f64 - m;
            let s = if t == 0.0 {
                2.0 * fc
            } else {
                (2.0 * PI * fc * t).sin() / (PI * t)
            };
            s * w[i]
        })
        .collect();
    let g: f64 = h.iter().sum();
    h.iter_mut().for_each(|x| *x /= g);
    h
}
