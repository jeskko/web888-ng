// SPDX-License-Identifier: GPL-3.0-or-later
//! The signal identifier (M8 item 8): names the mode of the signal at the
//! centre of 4.096 s of complex baseband at [`RATE`] (CW, RTTY and other
//! 2-FSK, PSK31, FT8, FT4, WSPR, ALE, STANAG 4285, MIL-STD-188-110A,
//! SSTV, HF fax, SSB voice, AM broadcast, a carrier, or nothing). A small
//! CNN trained on synthetic signals through random HF channels
//! (`tools/ml/sigid/`), run here without a framework; the weights (f16)
//! are embedded.
//!
//! Features (as `tools/ml/sigid/common.py`, which this must match):
//! * a spectrogram of two channels on one grid (381 frames × 256 bins):
//!   log power of a 256-point STFT (periodic Hann) every 64 samples, and
//!   of a 64-point one (each bin repeated 4 times; frames 2.. so that the
//!   centres nearly line up), 0 Hz in the middle;
//! * the fine spectrum: log of the 4096-point periodogram averaged over
//!   11 half-overlapping frames, the middle 2048 bins (±1.5 kHz);
//! * the autocorrelation |R(lag)| / R(0) at lags 6, 12, ... 2400 samples
//!   (1-400 ms).
//!
//! Log powers are relative to their median (the lower one), / 4, clamped
//! to [-3, 6]. Model: four 3×3 convolutions with 2×2 max pooling and a
//! fifth, the maximum and mean over time of each (channel, frequency);
//! 1-D convolutions on the fine spectrum (kernel 7, pool 4, three) and the
//! autocorrelation (kernel 5, pool 2, two), adaptive average pooling to
//! 16 and 25; two linear layers. Batch norm is folded into the
//! convolutions at export.

// index loops: the layers read as their equations
#![allow(clippy::needless_range_loop)]

pub mod measure;
pub mod scan;

use num_complex::Complex32;
use rustfft::FftPlanner;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Input rate (complex samples/s).
pub const RATE: f64 = 6000.0;
/// Input length (4.096 s).
pub const N: usize = 24576;
const LAGS: usize = 400;
const T: usize = (N - 256) / 64 + 1;

/// The classes, in the model's order.
pub const CLASSES: &[&str] = &[
    "noise",
    "carrier",
    "cw",
    "fsk",
    "psk31",
    "ft8",
    "ft4",
    "wspr",
    "ale",
    "stanag4285",
    "mil110a",
    "sstv",
    "fax",
    "ssb",
    "am",
    "interference",
    "radar",
    "hfdl",
];

static WEIGHTS: &[u8] = include_bytes!("../model/sigid.bin");

fn hann(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos()) as f32)
        .collect()
}

/// Log power spectra (fftshifted) of frames of n every hop.
fn stft_logp(x: &[Complex32], n: usize, hop: usize, frames: usize) -> Vec<Vec<f32>> {
    let fft = FftPlanner::new().plan_fft_forward(n);
    let w = hann(n);
    let mut buf = vec![Complex32::new(0.0, 0.0); n];
    (0..frames)
        .map(|i| {
            for k in 0..n {
                buf[k] = x[i * hop + k] * w[k];
            }
            fft.process(&mut buf);
            (0..n)
                .map(|j| (buf[(j + n / 2) % n].norm_sqr() + 1e-20).ln())
                .collect()
        })
        .collect()
}

/// (v - lower median) / 4, clamped to [-3, 6], in place.
fn norm(v: &mut [f32]) {
    let mut s = v.to_vec();
    let k = (s.len() - 1) / 2;
    let med = *s.select_nth_unstable_by(k, |a, b| a.total_cmp(b)).1;
    for x in v.iter_mut() {
        *x = ((*x - med) / 4.0).clamp(-3.0, 6.0);
    }
}

/// The three views: spectrogram (2 × T × 256), fine spectrum (2048),
/// autocorrelation (400).
pub fn features(x: &[Complex32]) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    assert_eq!(x.len(), N);
    let a = stft_logp(x, 256, 64, T);
    let b = stft_logp(x, 64, 64, T + 2);
    let mut spec = vec![0f32; 2 * T * 256];
    for i in 0..T {
        spec[i * 256..(i + 1) * 256].copy_from_slice(&a[i]);
        for j in 0..256 {
            spec[(T + i) * 256 + j] = b[i + 2][j / 4];
        }
    }
    let (s0, s1) = spec.split_at_mut(T * 256);
    norm(s0);
    norm(s1);
    // fine spectrum
    let fft = FftPlanner::new().plan_fft_forward(4096);
    let w = hann(4096);
    let mut p = vec![0f32; 4096];
    let mut buf = vec![Complex32::new(0.0, 0.0); 4096];
    let nf = (N - 4096) / 2048 + 1;
    for f in 0..nf {
        for k in 0..4096 {
            buf[k] = x[f * 2048 + k] * w[k];
        }
        fft.process(&mut buf);
        for k in 0..4096 {
            p[k] += buf[k].norm_sqr();
        }
    }
    let mut fine: Vec<f32> = (1024..3072)
        .map(|j| (p[(j + 2048) % 4096] / nf as f32 + 1e-20).ln())
        .collect();
    norm(&mut fine);
    // autocorrelation
    let mut planner = FftPlanner::new();
    let (f2, i2) = (
        planner.plan_fft_forward(2 * N),
        planner.plan_fft_inverse(2 * N),
    );
    let mut z = vec![Complex32::new(0.0, 0.0); 2 * N];
    z[..N].copy_from_slice(x);
    f2.process(&mut z);
    for v in z.iter_mut() {
        *v = Complex32::new(v.norm_sqr(), 0.0);
    }
    i2.process(&mut z);
    let r0 = z[0].re / (2 * N) as f32 + 1e-20;
    let ac = (1..=LAGS)
        .map(|j| z[6 * j].norm() / (2 * N) as f32 / r0)
        .collect();
    (spec, fine, ac)
}

/// Dot product with 8 accumulators (vectorises).
#[inline]
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    let mut acc = [0f32; 8];
    let (ca, ra) = a[..n].as_chunks::<8>();
    let (cb, rb) = b[..n].as_chunks::<8>();
    for (x, y) in ca.iter().zip(cb) {
        for k in 0..8 {
            acc[k] += x[k] * y[k];
        }
    }
    let mut s = acc.iter().sum::<f32>();
    for (x, y) in ra.iter().zip(rb) {
        s += x * y;
    }
    s
}

struct Layer {
    w: Vec<f32>,
    b: Vec<f32>,
    cout: usize,
    cin: usize,
    k: usize,
}

/// 3×3 convolution (padding 1) and ReLU of (cin, h, w) planes.
fn conv2d(l: &Layer, x: &[f32], h: usize, w: usize) -> Vec<f32> {
    let mut y = vec![0f32; l.cout * h * w];
    for o in 0..l.cout {
        let out = &mut y[o * h * w..(o + 1) * h * w];
        out.fill(l.b[o]);
        for c in 0..l.cin {
            let inp = &x[c * h * w..(c + 1) * h * w];
            for dy in 0..3 {
                for dx in 0..3 {
                    let wt = l.w[((o * l.cin + c) * 3 + dy) * 3 + dx];
                    // out[r][q] += wt * in[r + dy - 1][q + dx - 1]
                    let (r0, r1) = (1usize.saturating_sub(dy), (h + 1 - dy).min(h));
                    let (q0, q1) = (1usize.saturating_sub(dx), (w + 1 - dx).min(w));
                    for r in r0..r1 {
                        let src =
                            &inp[(r + dy - 1) * w + q0 + dx - 1..(r + dy - 1) * w + q1 + dx - 1];
                        let dst = &mut out[r * w + q0..r * w + q1];
                        for (d, s) in dst.iter_mut().zip(src) {
                            *d += wt * s;
                        }
                    }
                }
            }
        }
        out.iter_mut().for_each(|v| *v = v.max(0.0));
    }
    y
}

/// 2×2 max pooling (floor) of (c, h, w).
fn pool2d(x: &[f32], c: usize, h: usize, w: usize) -> (Vec<f32>, usize, usize) {
    let (h2, w2) = (h / 2, w / 2);
    let mut y = vec![0f32; c * h2 * w2];
    for k in 0..c {
        for r in 0..h2 {
            for q in 0..w2 {
                let i = k * h * w + 2 * r * w + 2 * q;
                y[(k * h2 + r) * w2 + q] = x[i].max(x[i + 1]).max(x[i + w]).max(x[i + w + 1]);
            }
        }
    }
    (y, h2, w2)
}

/// 1-D convolution (padding k/2), ReLU and max pooling by `p` of (cin, n).
fn conv1d(l: &Layer, x: &[f32], n: usize, p: usize) -> (Vec<f32>, usize) {
    let pad = l.k / 2;
    let mut col = vec![0f32; l.cin * l.k];
    let mut y = vec![0f32; l.cout * n];
    for j in 0..n {
        for c in 0..l.cin {
            for kk in 0..l.k {
                let i = (j + kk) as isize - pad as isize;
                col[c * l.k + kk] = if i >= 0 && (i as usize) < n {
                    x[c * n + i as usize]
                } else {
                    0.0
                };
            }
        }
        for o in 0..l.cout {
            y[o * n + j] =
                (l.b[o] + dot(&l.w[o * l.cin * l.k..(o + 1) * l.cin * l.k], &col)).max(0.0);
        }
    }
    let m = n / p;
    let mut z = vec![0f32; l.cout * m];
    for o in 0..l.cout {
        for j in 0..m {
            z[o * m + j] = y[o * n + j * p..o * n + (j + 1) * p]
                .iter()
                .copied()
                .fold(f32::MIN, f32::max);
        }
    }
    (z, m)
}

/// Adaptive average pooling (as PyTorch's) of (c, n) to (c, m).
fn adaptive_avg(x: &[f32], c: usize, n: usize, m: usize) -> Vec<f32> {
    let mut y = Vec::with_capacity(c * m);
    for k in 0..c {
        for i in 0..m {
            let (a, b) = (i * n / m, ((i + 1) * n).div_ceil(m));
            y.push(x[k * n + a..k * n + b].iter().sum::<f32>() / (b - a) as f32);
        }
    }
    y
}

fn linear(w: &[f32], b: &[f32], x: &[f32]) -> Vec<f32> {
    let d = x.len();
    (0..b.len())
        .map(|o| b[o] + dot(&w[o * d..(o + 1) * d], x))
        .collect()
}

pub struct SigId {
    spec: Vec<Layer>,
    fine: Vec<Layer>,
    ac: Vec<Layer>,
    h1: (Vec<f32>, Vec<f32>),
    h2: (Vec<f32>, Vec<f32>),
}

fn f16_to_f32(h: u16) -> f32 {
    let s = ((h >> 15) & 1) as u32;
    let e = ((h >> 10) & 0x1f) as u32;
    let m = (h & 0x3ff) as u32;
    let bits = if e == 0 {
        if m == 0 {
            s << 31
        } else {
            let mut e2 = 127 - 15 + 1;
            let mut m2 = m;
            while m2 & 0x400 == 0 {
                m2 <<= 1;
                e2 -= 1;
            }
            (s << 31) | ((e2 as u32) << 23) | ((m2 & 0x3ff) << 13)
        }
    } else if e == 31 {
        (s << 31) | (0xff << 23) | (m << 13)
    } else {
        (s << 31) | ((e + 127 - 15) << 23) | (m << 13)
    };
    f32::from_bits(bits)
}

type Tensor = (Vec<usize>, Vec<f32>);

/// Tensors by name (crates/cw's weight format, magic `W8SIGID1`).
fn parse(b: &[u8]) -> Option<HashMap<String, Tensor>> {
    if b.get(..8)? != b"W8SIGID1" {
        return None;
    }
    let mut p = 8;
    let u32_at = |p: &mut usize| -> Option<u32> {
        let v = u32::from_le_bytes(b.get(*p..*p + 4)?.try_into().ok()?);
        *p += 4;
        Some(v)
    };
    let n = u32_at(&mut p)?;
    let mut out = HashMap::new();
    for _ in 0..n {
        let ln = u16::from_le_bytes(b.get(p..p + 2)?.try_into().ok()?) as usize;
        p += 2;
        let name = std::str::from_utf8(b.get(p..p + ln)?).ok()?.to_string();
        p += ln;
        let nd = *b.get(p)? as usize;
        p += 1;
        let mut shape = Vec::new();
        for _ in 0..nd {
            shape.push(u32_at(&mut p)? as usize);
        }
        let cnt: usize = shape.iter().product();
        let vals = b
            .get(p..p + 2 * cnt)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| f16_to_f32(u16::from_le_bytes([c[0], c[1]])))
            .collect();
        p += 2 * cnt;
        out.insert(name, (shape, vals));
    }
    Some(out)
}

impl SigId {
    pub fn load(bytes: &[u8]) -> Option<SigId> {
        let mut t = parse(bytes)?;
        let mut layer = |name: &str| -> Option<Layer> {
            let (s, w) = t.remove(&format!("{name}.weight"))?;
            let (_, b) = t.remove(&format!("{name}.bias"))?;
            Some(Layer {
                cout: s[0],
                cin: s.get(1).copied().unwrap_or(0),
                k: s.get(2).copied().unwrap_or(1),
                w,
                b,
            })
        };
        let spec = ["spec.0.0", "spec.1.0", "spec.2.0", "spec.3.0", "spec.4"]
            .iter()
            .map(|n| layer(n))
            .collect::<Option<Vec<_>>>()?;
        let fine = ["fine.0.0", "fine.1.0", "fine.2.0"]
            .iter()
            .map(|n| layer(n))
            .collect::<Option<Vec<_>>>()?;
        let ac = ["ac.0.0", "ac.1.0"]
            .iter()
            .map(|n| layer(n))
            .collect::<Option<Vec<_>>>()?;
        let h1 = layer("head.0")?;
        let h2 = layer("head.3")?;
        Some(SigId {
            spec,
            fine,
            ac,
            h1: (h1.w, h1.b),
            h2: (h2.w, h2.b),
        })
    }

    /// The embedded model.
    pub fn get() -> &'static SigId {
        static NET: OnceLock<SigId> = OnceLock::new();
        NET.get_or_init(|| SigId::load(WEIGHTS).expect("the embedded model loads"))
    }

    /// The logits of N samples at RATE.
    pub fn logits(&self, x: &[Complex32]) -> Vec<f32> {
        let (spec, fine, ac) = features(x);
        // spectrogram: (2, T, 256)
        let (mut h, mut r, mut q, mut c) = (spec, T, 256usize, 2usize);
        for (i, l) in self.spec.iter().enumerate() {
            h = conv2d(l, &h, r, q);
            c = l.cout;
            if i < 4 {
                (h, r, q) = pool2d(&h, c, r, q);
            }
        }
        let mut v = Vec::with_capacity(2 * c * q);
        for k in 0..c {
            for f in 0..q {
                v.push(
                    (0..r)
                        .map(|t| h[(k * r + t) * q + f])
                        .fold(f32::MIN, f32::max),
                );
            }
        }
        for k in 0..c {
            for f in 0..q {
                v.push((0..r).map(|t| h[(k * r + t) * q + f]).sum::<f32>() / r as f32);
            }
        }
        // fine spectrum: 2048 -> 32, then 16
        let (mut g, mut n) = (fine, 2048usize);
        for l in &self.fine {
            (g, n) = conv1d(l, &g, n, 4);
        }
        v.extend(adaptive_avg(&g, self.fine[2].cout, n, 16));
        // autocorrelation: 400 -> 100, then 25
        let (mut a, mut n) = (ac, LAGS);
        for l in &self.ac {
            (a, n) = conv1d(l, &a, n, 2);
        }
        v.extend(adaptive_avg(&a, self.ac[1].cout, n, 25));
        let hid: Vec<f32> = linear(&self.h1.0, &self.h1.1, &v)
            .into_iter()
            .map(|x| x.max(0.0))
            .collect();
        linear(&self.h2.0, &self.h2.1, &hid)
    }
}

/// The classes with their probabilities, most likely first, for N
/// samples at RATE (the signal of interest near 0 Hz).
pub fn identify(x: &[Complex32]) -> Vec<(&'static str, f32)> {
    let l = SigId::get().logits(x);
    let m = l.iter().copied().fold(f32::MIN, f32::max);
    let e: Vec<f32> = l.iter().map(|v| (v - m).exp()).collect();
    let s: f32 = e.iter().sum();
    let mut out: Vec<(&'static str, f32)> = CLASSES
        .iter()
        .copied()
        .zip(e.iter().map(|v| v / s))
        .collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The network as PyTorch computes it (tools/ml/sigid/export.py).
    #[test]
    fn matches_pytorch() {
        let b: &[u8] = include_bytes!("../model/sigid-ref.bin");
        assert_eq!(&b[..8], b"W8SIDREF");
        let rd = |p: usize| u32::from_le_bytes(b[p..p + 4].try_into().unwrap()) as usize;
        let f = |p: usize| f32::from_le_bytes(b[p..p + 4].try_into().unwrap());
        let n = rd(8);
        let x: Vec<Complex32> = (0..n)
            .map(|i| Complex32::new(f(12 + 8 * i), f(16 + 8 * i)))
            .collect();
        let p = 12 + 8 * n;
        let c = rd(p);
        let l = SigId::get().logits(&x);
        assert_eq!(l.len(), c);
        let want: Vec<f32> = (0..c).map(|k| f(p + 4 + 4 * k)).collect();
        let worst = l
            .iter()
            .zip(&want)
            .map(|(a, b)| (a - b).abs())
            .fold(0f32, f32::max);
        assert!(
            worst < 0.05,
            "max logit difference {worst}: {l:?} vs {want:?}"
        );
        let arg = |v: &[f32]| (0..v.len()).max_by(|&i, &j| v[i].total_cmp(&v[j])).unwrap();
        assert_eq!(arg(&l), arg(&want));
    }
}
