// SPDX-License-Identifier: GPL-3.0-or-later
//! Test signals: BPSK as PSK31 transmitters make it. Each symbol is a
//! Hann pulse two symbols long; neighbours overlap so a steady phase keeps
//! a steady amplitude and a reversal passes through zero.

use crate::varicode;

/// Symbol amplitudes (±1) of `text`: idle reversals before, the text, a
/// few reversals and a steady carrier after (as transmitters end).
pub fn symbols(text: &str) -> Vec<f32> {
    let bits: Vec<u8> = [
        vec![0u8; 32],
        varicode::encode(text),
        vec![0u8; 8],
        vec![1u8; 16],
    ]
    .concat();
    let mut a = 1.0f32;
    bits.iter()
        .map(|&b| {
            if b == 0 {
                a = -a;
            }
            a
        })
        .collect()
}

/// The baseband envelope of `syms` at `fs` (real: BPSK).
pub fn baseband(syms: &[f32], baud: f64, fs: f64) -> Vec<f32> {
    let t = fs / baud;
    let n = ((syms.len() as f64 + 1.0) * t) as usize;
    let mut y = vec![0f32; n];
    for (k, &a) in syms.iter().enumerate() {
        // pulse centred at (k + 1) t, spanning ±t
        let c = (k as f64 + 1.0) * t;
        let i0 = (c - t).max(0.0).ceil() as usize;
        let i1 = ((c + t) as usize).min(n - 1);
        for (i, v) in y.iter_mut().enumerate().take(i1 + 1).skip(i0) {
            let x = (i as f64 - c) / t; // -1..1
            *v += a * (0.5 + 0.5 * (std::f64::consts::PI * x).cos()) as f32;
        }
    }
    y
}

/// Real audio: `text` at `baud` on carrier `fc` (Hz), amplitude `amp`,
/// starting phase `ph`.
pub fn audio(text: &str, baud: f64, fc: f64, fs: f64, amp: f32, ph: f64) -> Vec<f32> {
    let b = baseband(&symbols(text), baud, fs);
    b.iter()
        .enumerate()
        .map(|(i, &v)| {
            amp * v * (2.0 * std::f64::consts::PI * fc * i as f64 / fs + ph).cos() as f32
        })
        .collect()
}
