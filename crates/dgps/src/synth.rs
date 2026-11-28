// SPDX-License-Identifier: GPL-3.0-or-later
//! Test signals: RTCM messages as a beacon sends them, MSK on a carrier.

use crate::rtcm::{PREAMBLE, encode};
use std::f64::consts::PI;

/// The data words (24 bits) of a message: the two header words and `data`.
pub fn message(
    kind: u32,
    station: u32,
    zcount: u32,
    seq: u32,
    health: u32,
    data: &[u32],
) -> Vec<u32> {
    let h1 = (PREAMBLE << 16) | ((kind & 63) << 10) | (station & 1023);
    let h2 = ((zcount & 0x1fff) << 11)
        | ((seq & 7) << 8)
        | ((data.len() as u32 & 31) << 3)
        | (health & 7);
    [vec![h1, h2], data.to_vec()].concat()
}

/// A type 16 message's data words: `text`, three characters a word
/// (padded with NULs).
pub fn text_words(text: &str) -> Vec<u32> {
    let b: Vec<u8> = text.bytes().collect();
    b.chunks(3)
        .map(|c| {
            let g = |i: usize| u32::from(*c.get(i).unwrap_or(&0));
            (g(0) << 16) | (g(1) << 8) | g(2)
        })
        .collect()
}

/// The bits sent for these data words (parity chained, starting after a
/// word ending in 0, 0).
pub fn bits(words: &[u32]) -> Vec<bool> {
    let (mut p29, mut p30) = (0, 0);
    let mut out = Vec::new();
    for &d in words {
        let w = encode(d, p29, p30);
        out.extend((0..30).rev().map(|i| (w >> i) & 1 == 1));
        p29 = (w >> 1) & 1;
        p30 = w & 1;
    }
    out
}

/// Real audio: `bits` as MSK at `baud` on carrier `fc` (Hz), amplitude
/// `amp`, starting phase `ph`.
pub fn audio(bits: &[bool], baud: f64, fc: f64, fs: f64, amp: f32, ph: f64) -> Vec<f32> {
    let spb = fs / baud;
    let n = (bits.len() as f64 * spb) as usize;
    let mut phase = ph;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let b = bits[((i as f64 / spb) as usize).min(bits.len() - 1)];
        let dev = if b { 0.25 * baud } else { -0.25 * baud };
        out.push(amp * phase.cos() as f32);
        phase = (phase + 2.0 * PI * (fc + dev) / fs) % (2.0 * PI);
    }
    out
}
