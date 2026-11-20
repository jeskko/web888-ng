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

/// `Σ w[k] taps[k]`: complex samples by real taps (the same length), the
/// inner loop of a decimating filter. On 32-bit ARM, lengths that are a
/// multiple of 8 run NEON code (eight taps a step in four sums: about a
/// quarter of the scalar VFP code's time on the unit's Cortex-A9; the
/// armv7 target leaves NEON off for the compiler, as in `web888-fft`).
/// Other lengths and targets: four scalar sums.
pub fn dot_real(w: &[crate::C32], taps: &[f32]) -> crate::C32 {
    assert_eq!(w.len(), taps.len());
    #[cfg(target_arch = "arm")]
    if !w.is_empty() && w.len().is_multiple_of(8) {
        // SAFETY: both slices hold w.len() elements, a multiple of 8.
        return unsafe { neon::dot_real(w.as_ptr(), taps.as_ptr(), w.len()) };
    }
    let mut a = [0f32; 8];
    let (wc, wr) = w.as_chunks::<4>();
    let (tc, tr) = taps.as_chunks::<4>();
    for (h, t) in wc.iter().zip(tc) {
        a[0] += h[0].re * t[0];
        a[1] += h[0].im * t[0];
        a[2] += h[1].re * t[1];
        a[3] += h[1].im * t[1];
        a[4] += h[2].re * t[2];
        a[5] += h[2].im * t[2];
        a[6] += h[3].re * t[3];
        a[7] += h[3].im * t[3];
    }
    for (h, t) in wr.iter().zip(tr) {
        a[0] += h.re * t;
        a[1] += h.im * t;
    }
    crate::C32::new((a[0] + a[2]) + (a[4] + a[6]), (a[1] + a[3]) + (a[5] + a[7]))
}

#[cfg(target_arch = "arm")]
mod neon {
    use std::arch::asm;

    /// # Safety
    /// `w` (complex: 2n floats) and `t` (n floats) readable, n a multiple
    /// of 8 (> 0).
    pub unsafe fn dot_real(w: *const crate::C32, t: *const f32, n: usize) -> crate::C32 {
        // q8/q9: re/im sums of taps 0-3 of each step, q10/q11 of taps 4-7;
        // out: re sums (4) then im sums (4)
        let mut out = [0f32; 8];
        unsafe {
            asm!(
                ".fpu neon",
                "vpush {{d16-d31}}",
                "vmov.i32 q8, #0",
                "vmov.i32 q9, #0",
                "vmov.i32 q10, #0",
                "vmov.i32 q11, #0",
                "2:",
                "vld2.32 {{d0-d3}}, [{w}]!",
                "vld1.32 {{d4-d5}}, [{t}]!",
                "vld2.32 {{d6-d9}}, [{w}]!",
                "vld1.32 {{d10-d11}}, [{t}]!",
                "vmla.f32 q8, q0, q2",
                "vmla.f32 q9, q1, q2",
                "vmla.f32 q10, q3, q5",
                "vmla.f32 q11, q4, q5",
                "subs {c}, {c}, #1",
                "bne 2b",
                "vadd.f32 q8, q8, q10",
                "vadd.f32 q9, q9, q11",
                "vst1.32 {{d16-d19}}, [{o}]",
                "vpop {{d16-d31}}",
                c = inout(reg) n / 8 => _,
                w = inout(reg) w => _,
                t = inout(reg) t => _,
                o = in(reg) out.as_mut_ptr(),
                out("d0") _, out("d1") _, out("d2") _, out("d3") _,
                out("d4") _, out("d5") _, out("d6") _, out("d7") _,
                out("d8") _, out("d9") _, out("d10") _, out("d11") _,
            );
        }
        crate::C32::new(
            (out[0] + out[1]) + (out[2] + out[3]),
            (out[4] + out[5]) + (out[6] + out[7]),
        )
    }
}

#[cfg(test)]
mod dot_tests {
    use super::*;
    use crate::C32;

    /// Against a plain sum, for lengths on and off the fast path.
    #[test]
    fn dot_real_is_the_sum() {
        for n in [1usize, 7, 8, 16, 61, 128] {
            let w: Vec<C32> = (0..n)
                .map(|k| C32::new((k as f32 * 0.37).sin(), (k as f32 * 0.11).cos()))
                .collect();
            let t: Vec<f32> = (0..n).map(|k| 1.0 / (1.0 + k as f32)).collect();
            let want = w
                .iter()
                .zip(&t)
                .fold(C32::new(0.0, 0.0), |a, (x, y)| a + x * y);
            let got = dot_real(&w, &t);
            assert!((got - want).norm() < 1e-5, "{n}: {got} vs {want}");
        }
    }
}
