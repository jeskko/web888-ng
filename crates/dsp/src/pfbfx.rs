// SPDX-License-Identifier: GPL-3.0-or-later
//! Fixed-point model of the channelizer (`hw/docs/channelizer.md` §5
//! step 2): the integer datapath the FPGA would run, to choose its word
//! lengths against the float reference (`pfb`).
//!
//! * input: 16-bit ADC samples;
//! * prototype coefficients: `coef_bits` (DSP48 B port: 18);
//! * branch sums: exact (48-bit DSP accumulators), then rounded to the
//!   FFT's `data_bits` with `branch_shift`;
//! * the 4096-point real FFT as the FPGA would do it: the real branch sums
//!   packed into 2048 complex values, a radix-2 DIF FFT (rounding after
//!   each butterfly multiply, an optional rounded halving per stage set by
//!   `stage_shifts`), then the split into M/2 + 1 bins with twiddles of
//!   `twiddle_bits` and a final halving;
//! * saturation at `data_bits` everywhere, counted.
//!
//! The radix-2 butterfly order is that of a radix-2 single-path
//! delay-feedback pipeline; a radix-2² pipeline rounds at slightly
//! different points. Bit-exact test vectors come with the chosen Verilog
//! architecture; this model settles the widths.

use crate::C32;
use crate::pfb::PfbSpec;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FixedSpec {
    pub coef_bits: u32,
    /// FFT data width (real and imaginary parts each).
    pub data_bits: u32,
    pub twiddle_bits: u32,
    /// Right shift (rounded) from the exact branch sums to FFT data.
    pub branch_shift: u32,
    /// Bit s set: halve after FFT stage s (of 11 for 2048 points).
    pub stage_shifts: u32,
}

impl FixedSpec {
    /// The widths chosen with `examples/pfbfxmodel.rs`
    /// (`hw/docs/channelizer.md` §3.2): 20-bit coefficients (the DSP48's
    /// A port, ADC data on B), 25-bit FFT data, 18-bit twiddles, halving
    /// in the first four and the last four FFT stages. The error stays
    /// about 24 dB below the ADC noise in a bin, with nothing saturating
    /// for a tone at -1 dBFS.
    pub fn recommended(spec: &PfbSpec) -> Self {
        let stages = (spec.branches / 2).trailing_zeros();
        FixedSpec {
            coef_bits: 20,
            data_bits: 25,
            twiddle_bits: 18,
            branch_shift: 0,
            stage_shifts: 0b1111 | (0b1111 << (stages - 4)),
        }
    }

    /// A starting point: 18-bit coefficients and twiddles, 25-bit data,
    /// halving in every stage (about 10 dB worse than `recommended`).
    pub fn default_for(spec: &PfbSpec) -> Self {
        let stages = (spec.branches / 2).trailing_zeros();
        FixedSpec {
            coef_bits: 18,
            data_bits: 25,
            twiddle_bits: 18,
            branch_shift: 0, // set by `PfbFixed::new` if 0
            stage_shifts: (1 << stages) - 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Cx {
    re: i64,
    im: i64,
}

fn round_shift(v: i64, s: u32) -> i64 {
    if s == 0 { v } else { (v + (1 << (s - 1))) >> s }
}

pub struct PfbFixed {
    spec: PfbSpec,
    fx: FixedSpec,
    /// Coefficients as integers, and their scale (h = h_int / 2^coef_shift).
    h: Vec<i64>,
    coef_shift: u32,
    tw: Vec<Cx>,
    split_tw: Vec<Cx>,
    buf: Vec<i32>,
    since: usize,
    frame: u64,
    z: Vec<Cx>,
    out: Vec<C32>,
    /// Values clipped to `data_bits`.
    pub saturations: u64,
    /// Integer output to float (the float model's units).
    scale: f64,
}

impl PfbFixed {
    pub fn new(spec: PfbSpec, mut fx: FixedSpec) -> Self {
        let m = spec.branches;
        let n = m / 2;
        let stages = n.trailing_zeros();
        let hf = spec.prototype();
        let hmax = hf.iter().fold(0f32, |a, &b| a.max(b.abs())) as f64;
        // the largest coefficient just fits coef_bits
        let coef_shift = ((((1u64 << (fx.coef_bits - 1)) - 1) as f64 / hmax)
            .log2()
            .floor()) as u32;
        let h: Vec<i64> = hf
            .iter()
            .map(|&v| (v as f64 * (1u64 << coef_shift) as f64).round() as i64)
            .collect();
        if fx.branch_shift == 0 {
            // a full-scale input's branch sum may reach about
            // 2^15 · max_r Σ_t |h| · 2^coef_shift; fit it in data_bits - 2
            let worst = (0..m)
                .map(|r| (0..spec.taps).map(|t| h[r + t * m].abs()).sum::<i64>())
                .max()
                .unwrap() as f64
                * 32768.0;
            let bits = worst.log2().ceil() as i64;
            fx.branch_shift = (bits - (fx.data_bits as i64 - 2)).max(0) as u32;
        }
        let tq = ((1u64 << (fx.twiddle_bits - 1)) - 1) as f64;
        let tw = (0..n / 2)
            .map(|k| {
                let a = -2.0 * std::f64::consts::PI * k as f64 / n as f64;
                Cx {
                    re: (a.cos() * tq).round() as i64,
                    im: (a.sin() * tq).round() as i64,
                }
            })
            .collect();
        let split_tw = (0..=n)
            .map(|k| {
                let a = -2.0 * std::f64::consts::PI * k as f64 / m as f64;
                Cx {
                    re: (a.cos() * tq).round() as i64,
                    im: (a.sin() * tq).round() as i64,
                }
            })
            .collect();
        // the split's final halving only undoes its own sum of two terms
        let shifts = (fx.stage_shifts & ((1 << stages) - 1)).count_ones();
        // X_int ≈ X_float · 2^(15 + coef_shift - branch_shift - shifts)
        let scale = 2f64.powi(-(15 + coef_shift as i32 - fx.branch_shift as i32 - shifts as i32));
        PfbFixed {
            spec,
            fx,
            h,
            coef_shift,
            tw,
            split_tw,
            buf: Vec::new(),
            since: 0,
            frame: 0,
            z: vec![Cx::default(); n],
            out: vec![C32::default(); spec.bins()],
            saturations: 0,
            scale,
        }
    }

    pub fn fixed(&self) -> &FixedSpec {
        &self.fx
    }

    pub fn coef_shift(&self) -> u32 {
        self.coef_shift
    }

    /// Add 16-bit ADC samples; `frame` gets each frame's bins in the
    /// float model's units.
    pub fn push(&mut self, x: &[i16], mut frame: impl FnMut(&[C32])) {
        let d = self.spec.branches / 2;
        let l = self.spec.branches * self.spec.taps;
        let mut rest = x;
        while !rest.is_empty() {
            let n = (d - self.since).min(rest.len());
            self.buf.extend(rest[..n].iter().map(|&v| v as i32));
            rest = &rest[n..];
            self.since += n;
            if self.since == d {
                self.since = 0;
                if self.buf.len() >= l {
                    self.compute();
                    frame(&self.out);
                }
                if self.buf.len() > 4 * l {
                    let k = self.buf.len() - l;
                    self.buf.drain(..k);
                }
            }
        }
    }

    fn sat(&mut self, v: i64) -> i64 {
        let max = (1i64 << (self.fx.data_bits - 1)) - 1;
        if v > max {
            self.saturations += 1;
            max
        } else if v < -max - 1 {
            self.saturations += 1;
            -max - 1
        } else {
            v
        }
    }

    /// (a · w) >> (twiddle_bits - 1), rounded.
    fn cmul(&self, a: Cx, w: Cx) -> Cx {
        let s = self.fx.twiddle_bits - 1;
        Cx {
            re: round_shift(a.re * w.re - a.im * w.im, s),
            im: round_shift(a.re * w.im + a.im * w.re, s),
        }
    }

    fn compute(&mut self) {
        let m = self.spec.branches;
        let n = m / 2;
        let l = m * self.spec.taps;
        let base = self.buf.len() - l;
        // branch sums, exact, then to FFT data; even/odd branches packed
        let mut u = vec![0i64; m];
        for (r, ur) in u.iter_mut().enumerate() {
            let mut acc = 0i64;
            for t in 0..self.spec.taps {
                acc += self.h[r + t * m] * self.buf[base + l - 1 - r - t * m] as i64;
            }
            *ur = round_shift(acc, self.fx.branch_shift);
        }
        for k in 0..n {
            let (re, im) = (self.sat(u[2 * k]), self.sat(u[2 * k + 1]));
            self.z[k] = Cx { re, im };
        }
        // radix-2 DIF, natural in, bit-reversed out
        let stages = n.trailing_zeros();
        let mut half = n / 2;
        for s in 0..stages {
            let shift = (self.fx.stage_shifts >> s) & 1 == 1;
            let step = n / (2 * half);
            for start in (0..n).step_by(2 * half) {
                for j in 0..half {
                    let a = self.z[start + j];
                    let b = self.z[start + j + half];
                    let mut sum = Cx {
                        re: a.re + b.re,
                        im: a.im + b.im,
                    };
                    let dif = Cx {
                        re: a.re - b.re,
                        im: a.im - b.im,
                    };
                    let mut dif = self.cmul(dif, self.tw[j * step]);
                    if shift {
                        sum = Cx {
                            re: round_shift(sum.re, 1),
                            im: round_shift(sum.im, 1),
                        };
                        dif = Cx {
                            re: round_shift(dif.re, 1),
                            im: round_shift(dif.im, 1),
                        };
                    }
                    self.z[start + j] = Cx {
                        re: self.sat(sum.re),
                        im: self.sat(sum.im),
                    };
                    self.z[start + j + half] = Cx {
                        re: self.sat(dif.re),
                        im: self.sat(dif.im),
                    };
                }
            }
            half /= 2;
        }
        // to natural order
        let bits = stages;
        let zr: Vec<Cx> = (0..n)
            .map(|k| self.z[(k as u32).reverse_bits().wrapping_shr(32 - bits) as usize])
            .collect();
        // split: X[k] = (Z[k] + Z*[n-k])/2 + W^k (Z[k] - Z*[n-k])/(2j)
        let odd = self.frame % 2 == 1;
        for k in 0..=n {
            let zk = zr[k % n];
            let zc = {
                let v = zr[(n - k) % n];
                Cx {
                    re: v.re,
                    im: -v.im,
                }
            };
            let e = Cx {
                re: zk.re + zc.re,
                im: zk.im + zc.im,
            };
            let o = Cx {
                re: zk.re - zc.re,
                im: zk.im - zc.im,
            };
            // o / j = (o.im, -o.re)
            let o = self.cmul(
                Cx {
                    re: o.im,
                    im: -o.re,
                },
                self.split_tw[k],
            );
            let x = Cx {
                re: round_shift(e.re + o.re, 1),
                im: round_shift(e.im + o.im, 1),
            };
            let x = Cx {
                re: self.sat(x.re),
                im: self.sat(x.im),
            };
            // the bank's output is the conjugate; odd bins flip on odd frames
            let mut c = C32::new(
                (x.re as f64 * self.scale) as f32,
                (-x.im as f64 * self.scale) as f32,
            );
            if odd && k % 2 == 1 {
                c = -c;
            }
            self.out[k] = c;
        }
        self.frame += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pfb::Pfb;

    /// A tone near full scale plus ADC-level noise, as 16-bit samples.
    fn adc(n: usize, f: f64, dbfs: f64, seed: u64) -> Vec<i16> {
        let mut s = seed;
        let a = 10f64.powf(dbfs / 20.0) * 32767.0;
        (0..n)
            .map(|i| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                let u = (s >> 11) as f64 / (1u64 << 53) as f64 - 0.5;
                let v = a * (2.0 * std::f64::consts::PI * f / 122.88e6 * i as f64).cos()
                    + 3.0 * 3.4641 * u;
                v.round().clamp(-32768.0, 32767.0) as i16
            })
            .collect()
    }

    #[test]
    fn matches_the_float_model_well_below_the_adc_noise() {
        let spec = PfbSpec::web888();
        let x = adc(2048 * 100, 7.074e6, -1.0, 3);
        let xf: Vec<f32> = x.iter().map(|&v| v as f32 / 32768.0).collect();
        let mut fl = Pfb::new(spec);
        let mut fx = PfbFixed::new(spec, FixedSpec::recommended(&spec));
        let mut a = Vec::new();
        let mut b = Vec::new();
        fl.push(&xf, |f| a.push(f.to_vec()));
        fx.push(&x, |f| b.push(f.to_vec()));
        assert_eq!(fx.saturations, 0);
        // per-bin noise (float) and error (fixed - float), away from the tone
        let k0 = spec.bin_of(7.074e6);
        let (mut noise, mut err) = (0f64, 0f64);
        for (fa, fb) in a.iter().zip(&b).skip(30) {
            for k in (1..spec.bins() - 1).filter(|k| k.abs_diff(k0) > 3) {
                noise += fa[k].norm_sqr() as f64;
                err += (fb[k] - fa[k]).norm_sqr() as f64;
            }
        }
        let below = 10.0 * (noise / err).log10();
        assert!(
            below > 20.0,
            "fixed-point error only {below:.1} dB below the ADC noise"
        );
        // and the tone itself agrees
        let (fa, fb) = (&a[60][k0], &b[60][k0]);
        assert!((fa - fb).norm() / fa.norm() < 1e-4, "{fa} {fb}");
    }
}
