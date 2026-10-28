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
//! Two FFT arithmetics ([`FftArch`]): radix-2 (a twiddle multiply after
//! every stage), and radix-2², the chosen Verilog architecture (an R2²SDF
//! pipeline, `hw/docs/channelizer.md` §3.6). Radix-2² does each pair of
//! stages as two butterflies, the second with a trivial -j, then one
//! combined twiddle multiply, so it rounds half as often. Its rounding
//! points are the pipeline's, so this model's radix-2² output is the
//! reference for bit-exact test vectors.

use crate::C32;
use crate::pfb::PfbSpec;

/// How the FFT's arithmetic is arranged (the order of its roundings).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FftArch {
    /// Radix-2 DIF: a rounded twiddle multiply in every stage.
    Radix2,
    /// Radix-2² DIF (R2²SDF): per stage pair, two butterflies and one
    /// combined twiddle multiply (skipped where the twiddle is 1).
    Radix22,
}

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
    pub arch: FftArch,
}

impl FixedSpec {
    /// The widths chosen with `examples/pfbfxmodel.rs`
    /// (`hw/docs/channelizer.md` §3.2): 20-bit coefficients (the DSP48's
    /// A port, ADC data on B), 25-bit FFT data, 18-bit twiddles, halving
    /// in the first four and the last four FFT stages, radix-2². The error stays
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
            arch: FftArch::Radix22,
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
            arch: FftArch::Radix2,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Cx {
    re: i64,
    im: i64,
}

/// A power sum as the hardware sends it (`pfb_power.v`): IEEE float32
/// bits, the mantissa truncated (not rounded).
pub fn power_f32(v: u64) -> u32 {
    if v == 0 {
        return 0;
    }
    let lz = v.leading_zeros();
    let x = v << lz;
    ((190 - lz) << 23) | ((x >> 40) as u32 & 0x7f_ffff)
}

/// A bin's |X|^2, exact.
pub fn bin_power(v: CxInt) -> u64 {
    (v.0 * v.0 + v.1 * v.1) as u64
}

fn round_shift(v: i64, s: u32) -> i64 {
    if s == 0 { v } else { (v + (1 << (s - 1))) >> s }
}

/// A complex integer (re, im).
pub type CxInt = (i64, i64);

/// One frame's integers at the datapath's stage boundaries, for bit-exact
/// test vectors (as (re, im) pairs).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameTaps {
    /// FFT input: branch sums rounded to data, even/odd branches packed.
    pub packed: Vec<(i64, i64)>,
    /// FFT output in its bit-reversed order (as the pipeline emits it).
    pub fft: Vec<(i64, i64)>,
    /// The M/2 + 1 bins: conjugated, odd bins negated on odd frames.
    pub bins: Vec<(i64, i64)>,
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
    /// Keep each frame's stage integers (`taps`).
    record: bool,
    taps: FrameTaps,
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
        // the whole circle: radix-2² uses W^e for e up to 3n/4
        let tw = (0..n)
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
            record: false,
            taps: FrameTaps::default(),
            scale,
        }
    }

    pub fn fixed(&self) -> &FixedSpec {
        &self.fx
    }

    pub fn coef_shift(&self) -> u32 {
        self.coef_shift
    }

    /// The prototype as integers (`coef_bits`), h[r + t·M] for branch r,
    /// tap t.
    pub fn coefs(&self) -> &[i64] {
        &self.h
    }

    /// The FFT twiddles W_n^k, k < n (n = M/2), and the split's W_M^k,
    /// k <= n, as `twiddle_bits` integers.
    pub fn twiddles(&self) -> (Vec<CxInt>, Vec<CxInt>) {
        let f = |v: &Vec<Cx>| v.iter().map(|c| (c.re, c.im)).collect();
        (f(&self.tw), f(&self.split_tw))
    }

    /// Keep each frame's stage integers, read with `taps` after a frame.
    pub fn record(&mut self, on: bool) {
        self.record = on;
    }

    /// The last frame's stage integers (with `record` on).
    pub fn taps(&self) -> &FrameTaps {
        &self.taps
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

    /// Radix-2 DIF in place: natural in, bit-reversed out.
    fn fft_radix2(&mut self) {
        let n = self.spec.branches / 2;
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
    }

    /// Halve (rounded) if stage `s` halves, then saturate.
    fn stage_out(&mut self, v: Cx, s: u32) -> Cx {
        let v = if (self.fx.stage_shifts >> s) & 1 == 1 {
            Cx {
                re: round_shift(v.re, 1),
                im: round_shift(v.im, 1),
            }
        } else {
            v
        };
        Cx {
            re: self.sat(v.re),
            im: self.sat(v.im),
        }
    }

    /// Radix-2² DIF in place, as an R2²SDF pipeline computes it: natural
    /// in, bit-reversed out (the same order as radix-2). For each block of
    /// L = 4q at a stage pair (s, s + 1), and r < q, the four values at
    /// r + {0, 1, 2, 3}·q go through BF2I (x0 ± x2, x1 ± x3), BF2II (the
    /// second with -j on the odd difference), then twiddles W_L^{0, 2r, r,
    /// 3r}, rounded once. An odd stage count ends in a plain butterfly.
    fn fft_radix22(&mut self) {
        let n = self.spec.branches / 2;
        let stages = n.trailing_zeros();
        let mut s = 0;
        let mut l = n;
        while s + 1 < stages {
            let q = l / 4;
            for start in (0..n).step_by(l) {
                for r in 0..q {
                    let i = [
                        start + r,
                        start + r + q,
                        start + r + 2 * q,
                        start + r + 3 * q,
                    ];
                    let x = i.map(|k| self.z[k]);
                    // BF2I
                    let a = [
                        Cx {
                            re: x[0].re + x[2].re,
                            im: x[0].im + x[2].im,
                        },
                        Cx {
                            re: x[1].re + x[3].re,
                            im: x[1].im + x[3].im,
                        },
                        Cx {
                            re: x[0].re - x[2].re,
                            im: x[0].im - x[2].im,
                        },
                        Cx {
                            re: x[1].re - x[3].re,
                            im: x[1].im - x[3].im,
                        },
                    ]
                    .map(|v| self.stage_out(v, s));
                    // BF2II; -j·a3 = (a3.im, -a3.re)
                    let mj = Cx {
                        re: a[3].im,
                        im: -a[3].re,
                    };
                    let y = [
                        Cx {
                            re: a[0].re + a[1].re,
                            im: a[0].im + a[1].im,
                        },
                        Cx {
                            re: a[0].re - a[1].re,
                            im: a[0].im - a[1].im,
                        },
                        Cx {
                            re: a[2].re + mj.re,
                            im: a[2].im + mj.im,
                        },
                        Cx {
                            re: a[2].re - mj.re,
                            im: a[2].im - mj.im,
                        },
                    ]
                    .map(|v| self.stage_out(v, s + 1));
                    // twiddles W_L^e = W_n^(e·n/L)
                    for (p, e) in [0, 2 * r, r, 3 * r].into_iter().enumerate() {
                        let k = e * (n / l);
                        let v = if k == 0 {
                            y[p]
                        } else {
                            let m = self.cmul(y[p], self.tw[k]);
                            Cx {
                                re: self.sat(m.re),
                                im: self.sat(m.im),
                            }
                        };
                        self.z[i[p]] = v;
                    }
                }
            }
            s += 2;
            l /= 4;
        }
        if s < stages {
            // the last stage alone: L = 2, twiddle 1
            for k in (0..n).step_by(2) {
                let (a, b) = (self.z[k], self.z[k + 1]);
                self.z[k] = self.stage_out(
                    Cx {
                        re: a.re + b.re,
                        im: a.im + b.im,
                    },
                    s,
                );
                self.z[k + 1] = self.stage_out(
                    Cx {
                        re: a.re - b.re,
                        im: a.im - b.im,
                    },
                    s,
                );
            }
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
        if self.record {
            self.taps.packed = self.z.iter().map(|c| (c.re, c.im)).collect();
        }
        let stages = n.trailing_zeros();
        match self.fx.arch {
            FftArch::Radix2 => self.fft_radix2(),
            FftArch::Radix22 => self.fft_radix22(),
        }
        if self.record {
            self.taps.fft = self.z.iter().map(|c| (c.re, c.im)).collect();
            self.taps.bins.clear();
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
            let flip = odd && k % 2 == 1;
            if flip {
                c = -c;
            }
            if self.record {
                let (re, im) = (x.re, -x.im);
                self.taps
                    .bins
                    .push(if flip { (-re, -im) } else { (re, im) });
            }
            self.out[k] = c;
        }
        self.frame += 1;
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn power_floats_are_truncated() {
        assert_eq!(power_f32(0), 0);
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..10_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let v = (x >> (x % 64)).max(1);
            let f = f32::from_bits(power_f32(v)) as f64;
            let v64 = v as f64;
            assert!(f <= v64 && f > v64 * (1.0 - 2f64.powi(-23)), "{v} -> {f}");
            if v < 1 << 24 {
                assert_eq!(f, v64);
            }
        }
    }

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
    fn radix22_matches_radix2_and_taps_are_the_output() {
        let spec = PfbSpec::web888();
        let x = adc(2048 * 40, 10.1e6, -3.0, 5);
        let mut r2 = PfbFixed::new(
            spec,
            FixedSpec {
                arch: FftArch::Radix2,
                ..FixedSpec::recommended(&spec)
            },
        );
        let mut r22 = PfbFixed::new(spec, FixedSpec::recommended(&spec));
        r22.record(true);
        let (mut a, mut b) = (Vec::new(), Vec::new());
        let mut taps = Vec::new();
        r2.push(&x, |f| a.push(f.to_vec()));
        r22.push(&x, |f| b.push(f.to_vec()));
        // (one frame's taps: the last)
        taps.push(r22.taps().clone());
        let last = b.last().unwrap();
        let t = &taps[0];
        assert_eq!(t.bins.len(), spec.bins());
        assert_eq!(t.packed.len(), spec.branches / 2);
        for (k, c) in last.iter().enumerate() {
            let s = r22.scale;
            assert_eq!(c.re, (t.bins[k].0 as f64 * s) as f32, "bin {k}");
        }
        // the two arithmetics agree to their rounding: a few LSBs
        let k0 = spec.bin_of(10.1e6);
        for (fa, fb) in a.iter().zip(&b).skip(4) {
            let d = (fa[k0] - fb[k0]).norm() / fa[k0].norm();
            assert!(d < 1e-4, "{d}");
        }
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
