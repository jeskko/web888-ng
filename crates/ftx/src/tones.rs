// SPDX-License-Identifier: GPL-3.0-or-later
//! Channel symbols of FT8 and FT4, and a GFSK synthesizer for tests and
//! for subtracting decoded signals.

use crate::{crc, ldpc};
use std::cell::RefCell;
use std::rc::Rc;

pub const FT8_COSTAS: [u8; 7] = [3, 1, 4, 0, 6, 5, 2];
pub const FT8_GRAY: [u8; 8] = [0, 1, 3, 2, 5, 6, 4, 7];
pub const FT8_SYMBOLS: usize = 79;

pub const FT4_COSTAS: [[u8; 4]; 4] = [[0, 1, 3, 2], [1, 0, 2, 3], [2, 3, 1, 0], [3, 2, 0, 1]];
pub const FT4_GRAY: [u8; 4] = [0, 1, 3, 2];
pub const FT4_SYMBOLS: usize = 105;
/// FT4 scrambles the 77 message bits with this sequence (MSB first).
pub const FT4_XOR: [u8; 10] = [0x4a, 0x5e, 0x89, 0xb4, 0xb0, 0x8a, 0x79, 0x55, 0xbe, 0x28];

pub fn ft4_scramble(m: u128) -> u128 {
    let mut x = 0u128;
    for b in FT4_XOR {
        x = x << 8 | b as u128;
    }
    m ^ (x >> 3)
}

/// The 79 FT8 tones (0..8) of a 77-bit message.
pub fn ft8(m: u128) -> [u8; FT8_SYMBOLS] {
    let c = ldpc::encode(crc::with_crc(m));
    let mut t = [0u8; FT8_SYMBOLS];
    for start in [0, 36, 72] {
        t[start..start + 7].copy_from_slice(&FT8_COSTAS);
    }
    for d in 0..58 {
        let v = (0..3).fold(0, |v, k| v << 1 | ldpc::get(&c, 3 * d + k) as usize);
        let pos = if d < 29 { 7 + d } else { 43 + d - 29 };
        t[pos] = FT8_GRAY[v];
    }
    t
}

/// The 105 FT4 tones (0..4) of a 77-bit message.
pub fn ft4(m: u128) -> [u8; FT4_SYMBOLS] {
    let c = ldpc::encode(crc::with_crc(ft4_scramble(m)));
    let mut t = [0u8; FT4_SYMBOLS];
    for (k, start) in [1, 34, 67, 100].into_iter().enumerate() {
        t[start..start + 4].copy_from_slice(&FT4_COSTAS[k]);
    }
    for d in 0..87 {
        let v = (0..2).fold(0, |v, k| v << 1 | ldpc::get(&c, 2 * d + k) as usize);
        let pos = match d {
            0..29 => 5 + d,
            29..58 => 38 + d - 29,
            _ => 71 + d - 58,
        };
        t[pos] = FT4_GRAY[v];
    }
    t
}

/// GFSK pulse: a rectangular symbol smoothed by a Gaussian of
/// bandwidth-time product `bt` (2.0 for FT8, 1.0 for FT4), at `t` in
/// symbol periods from the symbol's centre.
fn gfsk_pulse(bt: f64, t: f64) -> f64 {
    let c = std::f64::consts::PI * (2.0 / std::f64::consts::LN_2).sqrt();
    0.5 * (erf(c * bt * (t + 0.5)) - erf(c * bt * (t - 0.5)))
}

fn erf(x: f64) -> f64 {
    // Abramowitz-Stegun 7.1.26 is too coarse for spectra; use a series /
    // continued fraction split.
    if x.abs() < 2.5 {
        let mut sum = x;
        let mut term = x;
        let x2 = x * x;
        for n in 1..60 {
            term *= -x2 / n as f64;
            sum += term / (2 * n + 1) as f64;
        }
        sum * 2.0 / std::f64::consts::PI.sqrt()
    } else {
        let s = x.signum();
        let x = x.abs();
        // erfc continued fraction (Lentz)
        let mut f = x;
        let mut c = x;
        let mut d = 0.0;
        for k in 1..80 {
            let a = k as f64 / 2.0;
            d = 1.0 / (x + a * d);
            c = x + a / c;
            f *= c * d;
        }
        let erfc = (-x * x).exp() / (f * std::f64::consts::PI.sqrt());
        s * (1.0 - erfc)
    }
}

/// The GFSK frequency pulse over 3 symbols, `nsps` samples each; built
/// once per shape and thread.
fn pulse_table(bt: f64, nsps: usize) -> Rc<Vec<f32>> {
    /// (bt as bits, nsps, table)
    type Entry = (u64, usize, Rc<Vec<f32>>);
    thread_local! {
        static CACHE: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
    }
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if let Some((_, _, t)) = c.iter().find(|(b, n, _)| *b == bt.to_bits() && *n == nsps) {
            return t.clone();
        }
        let t: Rc<Vec<f32>> = Rc::new(
            (0..3 * nsps)
                .map(|i| gfsk_pulse(bt, (i as f64 - 1.5 * nsps as f64) / nsps as f64) as f32)
                .collect(),
        );
        c.push((bt.to_bits(), nsps, t.clone()));
        t
    })
}

/// cos and sin of 2 pi x for x in cycles, from a table (error ~1e-6).
fn cis_cycles(x: f64) -> (f32, f32) {
    const N: usize = 4096;
    static TABLE: std::sync::OnceLock<Vec<(f32, f32)>> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        (0..=N)
            .map(|i| {
                let a = 2.0 * std::f64::consts::PI * i as f64 / N as f64;
                (a.cos() as f32, a.sin() as f32)
            })
            .collect()
    });
    let f = (x - x.floor()) * N as f64;
    let i = f as usize;
    let w = (f - i as f64) as f32;
    let (c0, s0) = t[i];
    let (c1, s1) = t[i + 1];
    (c0 + (c1 - c0) * w, s0 + (s1 - s0) * w)
}

/// Phase-continuous GFSK: calls `out(i, cos, sin)` with the envelope
/// applied, for each sample of `tones` at `rate`: tone spacing and
/// symbol rate `baud`, lowest tone at `f0` Hz, first and last symbol
/// ramped over 1/8 symbol.
fn gfsk(
    tones: &[u8],
    baud: f64,
    bt: f64,
    f0: f64,
    rate: f64,
    mut out: impl FnMut(usize, f32, f32),
) {
    let nsps = (rate / baud).round() as usize;
    let pulse = pulse_table(bt, nsps);
    let n = tones.len();
    let total = n * nsps;
    let r = nsps / 8;
    // Tones of the symbols before, at and after each sample; the first
    // and last tones continue into a guard symbol on each side.
    let tone = |j: isize| tones[j.clamp(0, n as isize - 1) as usize] as f32;
    // The pulse of symbol j covers its samples from nsps before to nsps
    // after; sample i (of symbol i / nsps) is at offset i % nsps + nsps.
    let freq = |i: usize| {
        let j = (i / nsps) as isize;
        let k = i % nsps;
        let dev =
            tone(j - 1) * pulse[k + 2 * nsps] + tone(j) * pulse[k + nsps] + tone(j + 1) * pulse[k];
        f0 + baud * dev as f64
    };
    let mut cycles = 0f64;
    let mut f_here = freq(0);
    for i in 0..total {
        let env = if i < r {
            0.5 * (1.0 - (std::f32::consts::PI * i as f32 / r as f32).cos())
        } else if i >= total - r {
            0.5 * (1.0 - (std::f32::consts::PI * (total - i) as f32 / r as f32).cos())
        } else {
            1.0
        };
        let (c, s) = cis_cycles(cycles);
        out(i, env * c, env * s);
        // Trapezoidal phase: at a few samples per symbol (200 S/s), the
        // frequency at the sample's start alone leaves a phase error of
        // half a sample's worth of the tone.
        let f_next = if i + 1 < total { freq(i + 1) } else { f_here };
        cycles += (f_here + f_next) / (2.0 * rate);
        f_here = f_next;
    }
}

/// The tones as a real signal of `len` samples, starting at `start`.
#[allow(clippy::too_many_arguments)]
pub fn synth(
    tones: &[u8],
    baud: f64,
    bt: f64,
    f0: f64,
    rate: f64,
    start: usize,
    len: usize,
    amp: f32,
) -> Vec<f32> {
    let mut out = vec![0f32; len];
    gfsk(tones, baud, bt, f0, rate, |i, _, s| {
        if let Some(o) = out.get_mut(start + i) {
            *o = amp * s;
        }
    });
    out
}

/// The tones as a complex signal exp(j phase), unit amplitude.
pub fn synth_complex(
    tones: &[u8],
    baud: f64,
    bt: f64,
    f0: f64,
    rate: f64,
) -> Vec<num_complex::Complex32> {
    let nsps = (rate / baud).round() as usize;
    let mut out = vec![num_complex::Complex32::default(); tones.len() * nsps];
    gfsk(tones, baud, bt, f0, rate, |i, c, s| {
        out[i] = num_complex::Complex32::new(c, s)
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ft8_tones_match_ft8code() {
        let m = crate::msg::pack("CQ K1ABC FN42").unwrap();
        let s: String = ft8(m).iter().map(|t| (b'0' + t) as char).collect();
        // ft8code "CQ K1ABC FN42": sync, data, sync, data, sync
        let want = [
            "3140652",
            "00000000100547670460602153343",
            "3140652",
            "73601104751700733474545513354",
            "3140652",
        ];
        assert_eq!(s, want.concat());
    }

    #[test]
    fn erf_is_accurate() {
        assert!((erf(0.5) - 0.520_499_877_813_046_5).abs() < 1e-12);
        assert!((erf(3.0) - 0.999_977_909_503_001_4).abs() < 1e-12);
        assert!((erf(-1.2) + 0.910_313_978_229_635_4).abs() < 1e-12);
    }
}
