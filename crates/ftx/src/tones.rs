// SPDX-License-Identifier: GPL-3.0-or-later
//! Channel symbols of FT8 and FT4, and a GFSK synthesizer for tests and
//! for subtracting decoded signals.

use crate::{crc, ldpc};

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

/// Phase-continuous GFSK: per-sample phase (radians) and envelope of
/// `tones` at `rate`, tone spacing and symbol rate `baud`, lowest tone at
/// `f0` Hz, first and last symbol ramped over 1/8 symbol.
fn gfsk(tones: &[u8], baud: f64, bt: f64, f0: f64, rate: f64) -> (Vec<f64>, Vec<f64>) {
    let nsps = (rate / baud).round() as usize;
    let n = tones.len();
    // Frequency deviation per sample in tone units, with a guard symbol
    // on each side.
    let pulse: Vec<f64> = (0..3 * nsps)
        .map(|i| gfsk_pulse(bt, (i as f64 - 1.5 * nsps as f64) / nsps as f64))
        .collect();
    let mut dev = vec![0f64; (n + 2) * nsps];
    for (j, &tone) in tones.iter().enumerate() {
        for (i, p) in pulse.iter().enumerate() {
            dev[j * nsps + i] += tone as f64 * p;
        }
    }
    // The first and last tones continue into the guard symbols.
    for i in 0..2 * nsps {
        dev[i] += tones[0] as f64 * pulse[i + nsps];
        // (the pulse's tail before its table starts is negligible)
        if i >= nsps {
            dev[n * nsps + i] += tones[n - 1] as f64 * pulse[i - nsps];
        }
    }
    let total = n * nsps;
    let r = nsps / 8;
    let mut phase = Vec::with_capacity(total);
    let mut env = Vec::with_capacity(total);
    let mut ph = 0f64;
    for i in 0..total {
        phase.push(ph);
        ph += 2.0 * std::f64::consts::PI * (f0 + baud * dev[i + nsps]) / rate;
        env.push(if i < r {
            0.5 * (1.0 - (std::f64::consts::PI * i as f64 / r as f64).cos())
        } else if i >= total - r {
            0.5 * (1.0 - (std::f64::consts::PI * (total - i) as f64 / r as f64).cos())
        } else {
            1.0
        });
    }
    (phase, env)
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
    let (phase, env) = gfsk(tones, baud, bt, f0, rate);
    let mut out = vec![0f32; len];
    for (i, (p, e)) in phase.iter().zip(&env).enumerate() {
        if let Some(o) = out.get_mut(start + i) {
            *o = amp * (e * p.sin()) as f32;
        }
    }
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
    let (phase, env) = gfsk(tones, baud, bt, f0, rate);
    phase
        .iter()
        .zip(&env)
        .map(|(p, e)| num_complex::Complex32::new((e * p.cos()) as f32, (e * p.sin()) as f32))
        .collect()
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
