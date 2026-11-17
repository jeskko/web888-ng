// SPDX-License-Identifier: GPL-3.0-or-later
//! FFT plans for the server. On 32-bit ARM, power-of-two sizes from 16 up
//! run a radix-4 Stockham FFT whose butterflies are NEON code, four at a
//! time (rustfft has no NEON path on ARMv7; its scalar VFP code was about
//! a third of the FT8 decoder's time on the unit's Cortex-A9). Other sizes
//! and other targets use rustfft. Results are those of a DFT to float
//! rounding, not rustfft's bit for bit.
//!
//! The NEON code is inline assembly with the `.fpu neon` directive: the
//! armv7 target leaves NEON off for the compiler, and Rust's ARM NEON
//! intrinsics are unstable. It uses d0-d31 (q0-q15): d0-d15 are declared
//! clobbered; d16-d31 can't be named on a target without them for the
//! compiler (the armv7 musl target), and a target with NEON on (the OS
//! image's, cortexa9 NEON) may keep values there, so each call saves and
//! restores them on the stack.
//!
//! The algorithm (Stockham, radix 4 then one radix-2 stage when log2 n is
//! odd): with `ns` the length of the transforms built so far, stage input
//! `x` and output `y`, for each j < n/r with k = j mod ns:
//! `v[q] = x[j + q n/r] w^(q k)` (w = e^(-+2 pi i / (ns r))), an r-point
//! DFT of `v`, and `y[(j / ns) ns r + k + q ns] = v'[q]`. Four consecutive
//! j share a group, so loads, twiddles and stores are contiguous, except
//! the first stage (ns = 1), whose outputs interleave and are shuffled in
//! registers.

use num_complex::Complex32 as C32;
use rustfft::{Fft, FftDirection, FftPlanner, Length};
use std::sync::{Arc, Mutex, OnceLock};

/// A plan for `n` points, forward or inverse, unnormalised (rustfft's
/// convention).
///
/// `WEB888_FFT=stockham` takes the Stockham code elsewhere too (its scalar
/// form, the same operations in the same order as the NEON code): for
/// scoring the decoders with it on a desktop.
pub fn plan(n: usize, inverse: bool) -> Arc<dyn Fft<f32>> {
    static FORCE: OnceLock<bool> = OnceLock::new();
    let force = *FORCE.get_or_init(|| std::env::var("WEB888_FFT").is_ok_and(|v| v == "stockham"));
    if (cfg!(target_arch = "arm") || force) && Stockham::fits(n) {
        return Arc::new(Stockham::new(n, inverse));
    }
    static PLANNER: OnceLock<Mutex<FftPlanner<f32>>> = OnceLock::new();
    let mut p = PLANNER
        .get_or_init(|| Mutex::new(FftPlanner::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if inverse {
        p.plan_fft_inverse(n)
    } else {
        p.plan_fft_forward(n)
    }
}

struct Stage {
    /// Transform length before the stage.
    ns: usize,
    radix: usize,
    /// Per 4 k: radix 4: w1 re, w1 im, w2 re, w2 im, w3 re, w3 im (4 each);
    /// radix 2: w re, w im. Empty for ns = 1.
    tw: Vec<f32>,
}

/// The radix-4 Stockham FFT (NEON butterflies on ARM, the same steps in
/// scalar code elsewhere: the tests check the algorithm on any machine).
pub struct Stockham {
    n: usize,
    inverse: bool,
    stages: Vec<Stage>,
}

impl Stockham {
    /// Sizes it takes: powers of two from 16.
    pub fn fits(n: usize) -> bool {
        n >= 16 && n.is_power_of_two()
    }

    pub fn new(n: usize, inverse: bool) -> Self {
        assert!(Self::fits(n), "{n} points");
        let sign = if inverse { 1.0 } else { -1.0 };
        let mut stages = vec![];
        let mut ns = 1;
        while ns * 4 <= n {
            let mut tw = vec![];
            if ns > 1 {
                for k0 in (0..ns).step_by(4) {
                    for q in 1..4 {
                        let w = |k: usize| {
                            let a = sign * 2.0 * std::f64::consts::PI * (q * k) as f64
                                / (4 * ns) as f64;
                            (a.cos() as f32, a.sin() as f32)
                        };
                        tw.extend((k0..k0 + 4).map(|k| w(k).0));
                        tw.extend((k0..k0 + 4).map(|k| w(k).1));
                    }
                }
            }
            stages.push(Stage { ns, radix: 4, tw });
            ns *= 4;
        }
        if ns < n {
            // one radix-2 stage: ns = n / 2
            let mut tw = vec![];
            for k0 in (0..ns).step_by(4) {
                let w = |k: usize| {
                    let a = sign * 2.0 * std::f64::consts::PI * k as f64 / n as f64;
                    (a.cos() as f32, a.sin() as f32)
                };
                tw.extend((k0..k0 + 4).map(|k| w(k).0));
                tw.extend((k0..k0 + 4).map(|k| w(k).1));
            }
            stages.push(Stage { ns, radix: 2, tw });
        }
        Stockham { n, inverse, stages }
    }

    /// One transform of `x` (n points), `t` the other buffer. Leaves the
    /// result in `x`.
    fn run(&self, x: &mut [C32], t: &mut [C32]) {
        let n = self.n;
        debug_assert!(x.len() == n && t.len() >= n);
        let t = &mut t[..n];
        let mut in_x = true;
        for s in &self.stages {
            let (src, dst): (&[C32], &mut [C32]) =
                if in_x { (&*x, &mut *t) } else { (&*t, &mut *x) };
            match (s.radix, s.ns) {
                (4, 1) => first4(src, dst, self.inverse),
                (4, _) => stage4(src, dst, s, self.inverse),
                _ => stage2(src, dst, s),
            }
            in_x = !in_x;
        }
        if !in_x {
            x.copy_from_slice(t);
        }
    }
}

/// The first radix-4 stage (ns = 1: no twiddles; y[4j + q]).
fn first4(x: &[C32], y: &mut [C32], inverse: bool) {
    let m = x.len() / 4;
    #[cfg(target_arch = "arm")]
    // SAFETY: m / 4 blocks of four j: j + 3 + 3m < n = x.len(),
    // 4j + 15 < n = y.len()
    unsafe {
        neon::first4(x.as_ptr(), m, y.as_mut_ptr(), inverse);
    }
    #[cfg(not(target_arch = "arm"))]
    for j in 0..m {
        let v = [x[j], x[j + m], x[j + 2 * m], x[j + 3 * m]];
        let o = dft4(v, inverse);
        y[4 * j..4 * j + 4].copy_from_slice(&o);
    }
}

/// A radix-4 stage with ns >= 4.
fn stage4(x: &[C32], y: &mut [C32], s: &Stage, inverse: bool) {
    let n = x.len();
    let (m, ns) = (n / 4, s.ns);
    assert!(s.tw.len() == ns / 4 * 24);
    for g in 0..m / ns {
        #[cfg(target_arch = "arm")]
        // SAFETY: the group's ns / 4 blocks: g ns + ns - 1 + 3m < n,
        // 4 g ns + 4 ns - 1 < n, ns / 4 * 24 twiddles
        unsafe {
            neon::stage4(
                x.as_ptr().add(g * ns),
                m,
                s.tw.as_ptr(),
                y.as_mut_ptr().add(g * ns * 4),
                ns,
                inverse,
            );
        }
        #[cfg(not(target_arch = "arm"))]
        for k in 0..ns {
            let tw = &s.tw[k / 4 * 24..][..24];
            let l = k % 4;
            let w = |q: usize| C32::new(tw[(q - 1) * 8 + l], tw[(q - 1) * 8 + 4 + l]);
            let j = g * ns + k;
            let v = [
                x[j],
                x[j + m] * w(1),
                x[j + 2 * m] * w(2),
                x[j + 3 * m] * w(3),
            ];
            for (q, r) in dft4(v, inverse).into_iter().enumerate() {
                y[g * ns * 4 + k + q * ns] = r;
            }
        }
    }
}

/// The last stage, radix 2 (ns = n / 2).
fn stage2(x: &[C32], y: &mut [C32], s: &Stage) {
    let ns = s.ns;
    assert!(s.tw.len() == ns / 4 * 8);
    #[cfg(target_arch = "arm")]
    // SAFETY: ns / 4 blocks of four k: k + 3 + ns < n on both sides,
    // ns / 4 * 8 twiddles
    unsafe {
        neon::stage2(x.as_ptr(), ns, s.tw.as_ptr(), y.as_mut_ptr());
    }
    #[cfg(not(target_arch = "arm"))]
    for k in 0..ns {
        let tw = &s.tw[k / 4 * 8..][..8];
        let a = x[k];
        let b = x[k + ns] * C32::new(tw[k % 4], tw[4 + k % 4]);
        y[k] = a + b;
        y[k + ns] = a - b;
    }
}

/// A 4-point DFT (the NEON code's order of operations).
#[cfg(not(target_arch = "arm"))]
fn dft4(v: [C32; 4], inverse: bool) -> [C32; 4] {
    let (a0, a1) = (v[0] + v[2], v[0] - v[2]);
    let (a2, a3) = (v[1] + v[3], v[1] - v[3]);
    // -i a3 forward, +i a3 inverse
    let ja3 = C32::new(-a3.im, a3.re);
    let (y1, y3) = if inverse {
        (a1 + ja3, a1 - ja3)
    } else {
        (a1 - ja3, a1 + ja3)
    };
    [a0 + a2, y1, a0 - a2, y3]
}

#[cfg(target_arch = "arm")]
mod neon {
    use num_complex::Complex32 as C32;
    use std::arch::asm;

    /// The radix-4 butterfly on v0 (q0, q1), v1 (q14, q15), v2 (q8, q9),
    /// v3 (q10, q11), re and im of four lanes each. Out: y0 (q0, q1),
    /// y1 (q10, q11), y2 (q8, q9), y3 (q14, q15); forward (-i) when
    /// `$fwd` is the first macro arm.
    macro_rules! dft4 {
        (forward) => {
            concat!(
                "vadd.f32 q4, q0, q8\n",
                "vadd.f32 q5, q1, q9\n",
                "vsub.f32 q6, q0, q8\n",
                "vsub.f32 q7, q1, q9\n",
                "vadd.f32 q12, q14, q10\n",
                "vadd.f32 q13, q15, q11\n",
                "vsub.f32 q2, q14, q10\n",
                "vsub.f32 q3, q15, q11\n",
                "vadd.f32 q0, q4, q12\n",
                "vadd.f32 q1, q5, q13\n",
                "vsub.f32 q8, q4, q12\n",
                "vsub.f32 q9, q5, q13\n",
                // y1 = a1 - i a3 = (a1.re + a3.im, a1.im - a3.re)
                "vadd.f32 q10, q6, q3\n",
                "vsub.f32 q11, q7, q2\n",
                // y3 = a1 + i a3 = (a1.re - a3.im, a1.im + a3.re)
                "vsub.f32 q14, q6, q3\n",
                "vadd.f32 q15, q7, q2\n",
            )
        };
        (inverse) => {
            concat!(
                "vadd.f32 q4, q0, q8\n",
                "vadd.f32 q5, q1, q9\n",
                "vsub.f32 q6, q0, q8\n",
                "vsub.f32 q7, q1, q9\n",
                "vadd.f32 q12, q14, q10\n",
                "vadd.f32 q13, q15, q11\n",
                "vsub.f32 q2, q14, q10\n",
                "vsub.f32 q3, q15, q11\n",
                "vadd.f32 q0, q4, q12\n",
                "vadd.f32 q1, q5, q13\n",
                "vsub.f32 q8, q4, q12\n",
                "vsub.f32 q9, q5, q13\n",
                // y1 = a1 + i a3
                "vsub.f32 q10, q6, q3\n",
                "vadd.f32 q11, q7, q2\n",
                // y3 = a1 - i a3
                "vadd.f32 q14, q6, q3\n",
                "vsub.f32 q15, q7, q2\n",
            )
        };
    }

    /// Loads x[0], x[m], x[2m], x[3m] (four lanes each) into v0 (q0, q1),
    /// v1 (q14, q15), v2 (q8, q9), v3 (q10, q11).
    macro_rules! load4 {
        () => {
            concat!(
                "vld2.32 {{d0-d3}}, [{x}]\n",
                "add {t}, {x}, {s}\n",
                "vld2.32 {{d28-d31}}, [{t}]\n",
                "add {t}, {t}, {s}\n",
                "vld2.32 {{d16-d19}}, [{t}]\n",
                "add {t}, {t}, {s}\n",
                "vld2.32 {{d20-d23}}, [{t}]\n",
            )
        };
    }

    macro_rules! first4_fn {
        ($name:ident, $dir:ident) => {
            /// The whole first stage, m / 4 blocks of four j.
            ///
            /// # Safety
            /// x[0..4m] readable, y[0..4m] writable, m a multiple of 4 (> 0).
            pub unsafe fn $name(x: *const C32, m: usize, y: *mut C32) {
                unsafe {
                    asm!(
                        ".fpu neon",
                        "vpush {{d16-d31}}",
                        "2:",
                        load4!(),
                        dft4!($dir),
                        // re/im of each output to complex pairs: d0 = y0
                        // lane 0, d1 lane 1, d2 lane 2, d3 lane 3, ...
                        "vzip.32 q0, q1",
                        "vzip.32 q10, q11",
                        "vzip.32 q8, q9",
                        "vzip.32 q14, q15",
                        // y[4 l + q] = output q of lane l
                        "vst1.32 {{d0}}, [{y}]!",
                        "vst1.32 {{d20}}, [{y}]!",
                        "vst1.32 {{d16}}, [{y}]!",
                        "vst1.32 {{d28}}, [{y}]!",
                        "vst1.32 {{d1}}, [{y}]!",
                        "vst1.32 {{d21}}, [{y}]!",
                        "vst1.32 {{d17}}, [{y}]!",
                        "vst1.32 {{d29}}, [{y}]!",
                        "vst1.32 {{d2}}, [{y}]!",
                        "vst1.32 {{d22}}, [{y}]!",
                        "vst1.32 {{d18}}, [{y}]!",
                        "vst1.32 {{d30}}, [{y}]!",
                        "vst1.32 {{d3}}, [{y}]!",
                        "vst1.32 {{d23}}, [{y}]!",
                        "vst1.32 {{d19}}, [{y}]!",
                        "vst1.32 {{d31}}, [{y}]!",
                        "add {x}, {x}, #32",
                        "subs {c}, {c}, #1",
                        "bne 2b",
                        "vpop {{d16-d31}}",
                        c = inout(reg) m / 4 => _,
                        x = inout(reg) x => _,
                        s = in(reg) m * 8,
                        t = out(reg) _,
                        y = inout(reg) y => _,
                        out("d0") _, out("d1") _, out("d2") _, out("d3") _,
                        out("d4") _, out("d5") _, out("d6") _, out("d7") _,
                        out("d8") _, out("d9") _, out("d10") _, out("d11") _,
                        out("d12") _, out("d13") _, out("d14") _, out("d15") _,
                                            );
                }
            }
        };
    }

    macro_rules! stage4_fn {
        ($name:ident, $dir:ident) => {
            /// One group: ns / 4 blocks of four k.
            ///
            /// # Safety
            /// x[0..ns + 3m] readable, w[0..ns / 4 * 24] readable,
            /// y[0..4 ns] writable, ns a multiple of 4 (> 0).
            pub unsafe fn $name(x: *const C32, m: usize, w: *const f32, y: *mut C32, ns: usize) {
                unsafe {
                    asm!(
                        ".fpu neon",
                        "vpush {{d16-d31}}",
                        "2:",
                        load4!(),
                        // twiddles: w1 (q2, q3), w2 (q4, q5), w3 (q6, q7)
                        "vld1.32 {{d4-d7}}, [{w}]!",
                        "vld1.32 {{d8-d11}}, [{w}]!",
                        "vld1.32 {{d12-d15}}, [{w}]!",
                        // v1 *= w1 -> (q12, q13) -> (q14, q15)
                        "vmul.f32 q12, q14, q2",
                        "vmls.f32 q12, q15, q3",
                        "vmul.f32 q13, q14, q3",
                        "vmla.f32 q13, q15, q2",
                        "vmov q14, q12",
                        "vmov q15, q13",
                        // v2 *= w2 -> (q12, q13) -> (q8, q9)
                        "vmul.f32 q12, q8, q4",
                        "vmls.f32 q12, q9, q5",
                        "vmul.f32 q13, q8, q5",
                        "vmla.f32 q13, q9, q4",
                        "vmov q8, q12",
                        "vmov q9, q13",
                        // v3 *= w3 -> (q12, q13) -> (q10, q11)
                        "vmul.f32 q12, q10, q6",
                        "vmls.f32 q12, q11, q7",
                        "vmul.f32 q13, q10, q7",
                        "vmla.f32 q13, q11, q6",
                        "vmov q10, q12",
                        "vmov q11, q13",
                        dft4!($dir),
                        "vst2.32 {{d0-d3}}, [{y}]",
                        "add {t}, {y}, {o}",
                        "vst2.32 {{d20-d23}}, [{t}]",
                        "add {t}, {t}, {o}",
                        "vst2.32 {{d16-d19}}, [{t}]",
                        "add {t}, {t}, {o}",
                        "vst2.32 {{d28-d31}}, [{t}]",
                        "add {x}, {x}, #32",
                        "add {y}, {y}, #32",
                        "subs {c}, {c}, #1",
                        "bne 2b",
                        "vpop {{d16-d31}}",
                        c = inout(reg) ns / 4 => _,
                        x = inout(reg) x => _,
                        s = in(reg) m * 8,
                        w = inout(reg) w => _,
                        y = inout(reg) y => _,
                        o = in(reg) ns * 8,
                        t = out(reg) _,
                        out("d0") _, out("d1") _, out("d2") _, out("d3") _,
                        out("d4") _, out("d5") _, out("d6") _, out("d7") _,
                        out("d8") _, out("d9") _, out("d10") _, out("d11") _,
                        out("d12") _, out("d13") _, out("d14") _, out("d15") _,
                                            );
                }
            }
        };
    }

    first4_fn!(first4_fwd, forward);
    first4_fn!(first4_inv, inverse);
    stage4_fn!(stage4_fwd, forward);
    stage4_fn!(stage4_inv, inverse);

    /// # Safety
    /// As the macro-made functions.
    pub unsafe fn first4(x: *const C32, m: usize, y: *mut C32, inverse: bool) {
        unsafe {
            if inverse {
                first4_inv(x, m, y)
            } else {
                first4_fwd(x, m, y)
            }
        }
    }

    /// # Safety
    /// As the macro-made functions.
    pub unsafe fn stage4(
        x: *const C32,
        m: usize,
        w: *const f32,
        y: *mut C32,
        ns: usize,
        inverse: bool,
    ) {
        unsafe {
            if inverse {
                stage4_inv(x, m, w, y, ns)
            } else {
                stage4_fwd(x, m, w, y, ns)
            }
        }
    }

    /// The radix-2 stage, ns / 4 blocks of four k: y[k] = a + w b,
    /// y[k + ns] = a - w b.
    ///
    /// # Safety
    /// x[0..2 ns] readable, w[0..ns / 4 * 8] readable, y[0..2 ns]
    /// writable, ns a multiple of 4 (> 0).
    pub unsafe fn stage2(x: *const C32, ns: usize, w: *const f32, y: *mut C32) {
        unsafe {
            asm!(
            ".fpu neon",
                    "vpush {{d16-d31}}",
            "2:",
            "vld2.32 {{d0-d3}}, [{x}]",
            "add {t}, {x}, {o}",
            "vld2.32 {{d4-d7}}, [{t}]",
            "vld1.32 {{d8-d11}}, [{w}]!",
            // b *= w -> (q6, q7)
            "vmul.f32 q6, q2, q4",
            "vmls.f32 q6, q3, q5",
            "vmul.f32 q7, q2, q5",
            "vmla.f32 q7, q3, q4",
            "vadd.f32 q8, q0, q6",
            "vadd.f32 q9, q1, q7",
            "vsub.f32 q10, q0, q6",
            "vsub.f32 q11, q1, q7",
            "vst2.32 {{d16-d19}}, [{y}]",
            "add {t}, {y}, {o}",
            "vst2.32 {{d20-d23}}, [{t}]",
            "add {x}, {x}, #32",
            "add {y}, {y}, #32",
            "subs {c}, {c}, #1",
            "bne 2b",
                    "vpop {{d16-d31}}",
            c = inout(reg) ns / 4 => _,
            x = inout(reg) x => _,
            w = inout(reg) w => _,
            y = inout(reg) y => _,
            o = in(reg) ns * 8,
            t = out(reg) _,
            out("d0") _, out("d1") _, out("d2") _, out("d3") _,
            out("d4") _, out("d5") _, out("d6") _, out("d7") _,
            out("d8") _, out("d9") _, out("d10") _, out("d11") _,
            out("d12") _, out("d13") _, out("d14") _, out("d15") _,
                        );
        }
    }
}

impl Length for Stockham {
    fn len(&self) -> usize {
        self.n
    }
}

impl rustfft::Direction for Stockham {
    fn fft_direction(&self) -> FftDirection {
        if self.inverse {
            FftDirection::Inverse
        } else {
            FftDirection::Forward
        }
    }
}

impl Fft<f32> for Stockham {
    fn process_with_scratch(&self, buffer: &mut [C32], scratch: &mut [C32]) {
        let n = self.n;
        assert!(buffer.len().is_multiple_of(n) && scratch.len() >= n);
        for x in buffer.chunks_exact_mut(n) {
            self.run(x, scratch);
        }
    }

    fn process_outofplace_with_scratch(
        &self,
        input: &mut [C32],
        output: &mut [C32],
        scratch: &mut [C32],
    ) {
        let n = self.n;
        assert!(input.len() == output.len() && input.len().is_multiple_of(n));
        let _ = scratch;
        for (i, o) in input.chunks_exact_mut(n).zip(output.chunks_exact_mut(n)) {
            o.copy_from_slice(i);
            self.run(o, i);
        }
    }

    fn process_immutable_with_scratch(
        &self,
        input: &[C32],
        output: &mut [C32],
        scratch: &mut [C32],
    ) {
        let n = self.n;
        assert!(input.len() == output.len() && input.len().is_multiple_of(n) && scratch.len() >= n);
        for (i, o) in input.chunks_exact(n).zip(output.chunks_exact_mut(n)) {
            o.copy_from_slice(i);
            self.run(o, scratch);
        }
    }

    fn get_inplace_scratch_len(&self) -> usize {
        self.n
    }

    fn get_outofplace_scratch_len(&self) -> usize {
        0
    }

    fn get_immutable_scratch_len(&self) -> usize {
        self.n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signal(n: usize, seed: u32) -> Vec<C32> {
        let mut r = seed.wrapping_mul(2_654_435_761) | 1;
        let mut f = || {
            r ^= r << 13;
            r ^= r >> 17;
            r ^= r << 5;
            r as f32 / u32::MAX as f32 - 0.5
        };
        (0..n).map(|_| C32::new(f(), f())).collect()
    }

    /// Against rustfft, forward and inverse, every size from 16 to 64 k,
    /// several transforms in one buffer.
    #[test]
    fn matches_rustfft() {
        let mut p = FftPlanner::new();
        for log in 4..=16 {
            let n = 1usize << log;
            for inverse in [false, true] {
                let s = Stockham::new(n, inverse);
                let r = if inverse {
                    p.plan_fft_inverse(n)
                } else {
                    p.plan_fft_forward(n)
                };
                let x = signal(2 * n, log as u32 + inverse as u32 * 100);
                let mut a = x.clone();
                let mut b = x.clone();
                let mut scratch = vec![C32::default(); s.get_inplace_scratch_len()];
                s.process_with_scratch(&mut a, &mut scratch);
                r.process(&mut b);
                let scale = (n as f32).sqrt();
                let worst = a
                    .iter()
                    .zip(&b)
                    .map(|(a, b)| (a - b).norm())
                    .fold(0f32, f32::max);
                assert!(
                    worst < 2e-6 * scale * (log as f32),
                    "n {n} inverse {inverse}: {worst}"
                );
                // out of place and immutable agree
                let mut c = vec![C32::default(); 2 * n];
                s.process_immutable_with_scratch(&x, &mut c, &mut scratch);
                assert_eq!(c, a);
                let mut i = x.clone();
                let mut d = vec![C32::default(); 2 * n];
                s.process_outofplace_with_scratch(&mut i, &mut d, &mut []);
                assert_eq!(d, a);
            }
        }
    }

    #[test]
    fn plan_takes_any_size() {
        for n in [1, 5, 12, 16, 3200, 1024] {
            let p = plan(n, false);
            assert_eq!(p.len(), n);
            let mut x = signal(n, 7);
            p.process(&mut x);
        }
    }
}
