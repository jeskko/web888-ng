// SPDX-License-Identifier: GPL-3.0-or-later
//! The neural CW decoder (M8 item 7): a small CTC network trained on
//! synthetic CW (`tools/ml/cw/`), run here without a framework. It reads
//! the same channel as the classical decoder (complex baseband at
//! [`ENV_FS`], the carrier near 0 Hz).
//!
//! Features (as `tools/ml/cw/common.py`): a 16-point STFT (periodic Hann)
//! every 2 samples; log power of the 5 bins around 0 Hz, minus the 20th
//! percentile of the centre bin's, / 4, clamped to [-2, 8]. Model: three
//! 1-D convolutions (ReLU; the last two stride 2), a 2-layer bidirectional
//! GRU, a linear layer to the classes (0 = blank); greedy CTC.

// index loops: the layers read as their equations
#![allow(clippy::needless_range_loop)]

use crate::decode::ENV_FS;
use crate::dsp;
use num_complex::Complex32;
use std::collections::HashMap;
use std::sync::OnceLock;

const NFFT: usize = 16;
const HOP: usize = 2;
const BINS: [i32; 5] = [-2, -1, 0, 1, 2];

/// The classes (index 0 is the CTC blank).
pub const CLASSES: &[&str] = &[
    "_", " ", "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q",
    "R", "S", "T", "U", "V", "W", "X", "Y", "Z", "0", "1", "2", "3", "4", "5", "6", "7", "8", "9",
    ".", ",", "?", "/", "=", "+", "-", "'", "(", ")", ":", ";", "\"", "@", "!", "&", "<SK>",
    "<KN>", "<AS>", "<BK>", "<HH>",
];

static WEIGHTS: &[u8] = include_bytes!("../model/cwnet.bin");

/// Features: 5 rows of T values.
pub fn features(x: &[Complex32]) -> Vec<Vec<f32>> {
    if x.len() < NFFT {
        return vec![Vec::new(); BINS.len()];
    }
    let t = (x.len() - NFFT) / HOP + 1;
    let win: Vec<f32> = (0..NFFT)
        .map(|i| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / NFFT as f64).cos()) as f32)
        .collect();
    let basis: Vec<Vec<Complex32>> = BINS
        .iter()
        .map(|&k| {
            (0..NFFT)
                .map(|n| {
                    let a = -2.0 * std::f64::consts::PI * k as f64 * n as f64 / NFFT as f64;
                    Complex32::new(a.cos() as f32, a.sin() as f32)
                })
                .collect()
        })
        .collect();
    let mut lp = vec![vec![0f32; t]; BINS.len()];
    for i in 0..t {
        let fr = &x[i * HOP..i * HOP + NFFT];
        for (b, row) in basis.iter().enumerate() {
            let mut s = Complex32::new(0.0, 0.0);
            for n in 0..NFFT {
                s += fr[n] * win[n] * row[n];
            }
            lp[b][i] = (s.norm_sqr() + 1e-12).ln();
        }
    }
    let floor = dsp::percentile(&lp[2], 20.0);
    for row in &mut lp {
        for v in row.iter_mut() {
            *v = ((*v - floor) / 4.0).clamp(-2.0, 8.0);
        }
    }
    lp
}

struct Conv {
    w: Vec<f32>, // (out, in, k)
    b: Vec<f32>,
    cin: usize,
    cout: usize,
    k: usize,
    stride: usize,
}

impl Conv {
    /// x: cin rows of T; out: cout rows; ReLU.
    fn run(&self, x: &[Vec<f32>]) -> Vec<Vec<f32>> {
        let t = x[0].len();
        let pad = self.k / 2;
        let tout = (t + 2 * pad - self.k) / self.stride + 1;
        let mut y = vec![vec![0f32; tout]; self.cout];
        for (o, yo) in y.iter_mut().enumerate() {
            for (j, out) in yo.iter_mut().enumerate() {
                let mut s = self.b[o];
                for c in 0..self.cin {
                    let w = &self.w[(o * self.cin + c) * self.k..(o * self.cin + c + 1) * self.k];
                    for (kk, wk) in w.iter().enumerate() {
                        let i = (j * self.stride + kk) as isize - pad as isize;
                        if i >= 0 && (i as usize) < t {
                            s += wk * x[c][i as usize];
                        }
                    }
                }
                *out = s.max(0.0);
            }
        }
        y
    }
}

struct GruDir {
    wih: Vec<f32>, // (3H, in)
    whh: Vec<f32>, // (3H, H)
    bih: Vec<f32>,
    bhh: Vec<f32>,
    inp: usize,
    h: usize,
}

fn sigmoid(v: f32) -> f32 {
    1.0 / (1.0 + (-v).exp())
}

impl GruDir {
    /// x: T frames of `inp`; returns T frames of H (in time order).
    fn run(&self, x: &[Vec<f32>], reverse: bool) -> Vec<Vec<f32>> {
        let (n, h) = (x.len(), self.h);
        let mut out = vec![vec![0f32; h]; n];
        let mut hs = vec![0f32; h];
        let mut gi = vec![0f32; 3 * h];
        let mut gh = vec![0f32; 3 * h];
        for step in 0..n {
            let t = if reverse { n - 1 - step } else { step };
            for r in 0..3 * h {
                let wi = &self.wih[r * self.inp..(r + 1) * self.inp];
                gi[r] = self.bih[r] + wi.iter().zip(&x[t]).map(|(a, b)| a * b).sum::<f32>();
                let wh = &self.whh[r * h..(r + 1) * h];
                gh[r] = self.bhh[r] + wh.iter().zip(&hs).map(|(a, b)| a * b).sum::<f32>();
            }
            for j in 0..h {
                let r = sigmoid(gi[j] + gh[j]);
                let z = sigmoid(gi[h + j] + gh[h + j]);
                let nn = (gi[2 * h + j] + r * gh[2 * h + j]).tanh();
                hs[j] = (1.0 - z) * nn + z * hs[j];
            }
            out[t].copy_from_slice(&hs);
        }
        out
    }
}

pub struct CwNet {
    convs: [Conv; 3],
    gru: Vec<(GruDir, GruDir)>,
    fc_w: Vec<f32>,
    fc_b: Vec<f32>,
    classes: usize,
}

fn f16_to_f32(h: u16) -> f32 {
    let s = ((h >> 15) & 1) as u32;
    let e = ((h >> 10) & 0x1f) as u32;
    let m = (h & 0x3ff) as u32;
    let bits = if e == 0 {
        if m == 0 {
            s << 31
        } else {
            // subnormal: normalise
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

/// A tensor: shape, values.
type Tensor = (Vec<usize>, Vec<f32>);

/// Tensors of a cwnet.bin by name.
fn parse(b: &[u8]) -> Option<HashMap<String, Tensor>> {
    if b.get(..8)? != b"W8CWNET1" {
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

impl CwNet {
    pub fn load(bytes: &[u8]) -> Option<CwNet> {
        let mut t = parse(bytes)?;
        let mut take = |k: &str| t.remove(k);
        let conv = |w: (Vec<usize>, Vec<f32>), b: (Vec<usize>, Vec<f32>), stride| Conv {
            cout: w.0[0],
            cin: w.0[1],
            k: w.0[2],
            w: w.1,
            b: b.1,
            stride,
        };
        let convs = [
            conv(take("c1.weight")?, take("c1.bias")?, 1),
            conv(take("c2.weight")?, take("c2.bias")?, 2),
            conv(take("c3.weight")?, take("c3.bias")?, 2),
        ];
        let mut gru = Vec::new();
        for l in 0.. {
            let Some(wih) = take(&format!("gru.weight_ih_l{l}")) else {
                break;
            };
            let dir = |wih: (Vec<usize>, Vec<f32>),
                       whh: (Vec<usize>, Vec<f32>),
                       bih: (Vec<usize>, Vec<f32>),
                       bhh: (Vec<usize>, Vec<f32>)| GruDir {
                h: whh.0[1],
                inp: wih.0[1],
                wih: wih.1,
                whh: whh.1,
                bih: bih.1,
                bhh: bhh.1,
            };
            let f = dir(
                wih,
                take(&format!("gru.weight_hh_l{l}"))?,
                take(&format!("gru.bias_ih_l{l}"))?,
                take(&format!("gru.bias_hh_l{l}"))?,
            );
            let b = dir(
                take(&format!("gru.weight_ih_l{l}_reverse"))?,
                take(&format!("gru.weight_hh_l{l}_reverse"))?,
                take(&format!("gru.bias_ih_l{l}_reverse"))?,
                take(&format!("gru.bias_hh_l{l}_reverse"))?,
            );
            gru.push((f, b));
        }
        let (fs, fc_w) = take("fc.weight")?;
        let (_, fc_b) = take("fc.bias")?;
        Some(CwNet {
            convs,
            gru,
            fc_w,
            fc_b,
            classes: fs[0],
        })
    }

    /// The embedded model.
    pub fn get() -> &'static CwNet {
        static NET: OnceLock<CwNet> = OnceLock::new();
        NET.get_or_init(|| CwNet::load(WEIGHTS).expect("the embedded model loads"))
    }

    /// Log-probabilities (T' frames × classes) of a channel.
    pub fn forward(&self, x: &[Complex32]) -> Vec<Vec<f32>> {
        let mut h = features(x);
        if h[0].is_empty() {
            return Vec::new();
        }
        for c in &self.convs {
            h = c.run(&h);
        }
        // channels × T → T × channels
        let t = h[0].len();
        let mut seq: Vec<Vec<f32>> = (0..t)
            .map(|i| h.iter().map(|row| row[i]).collect())
            .collect();
        for (f, b) in &self.gru {
            let a = f.run(&seq, false);
            let z = b.run(&seq, true);
            seq = a
                .into_iter()
                .zip(z)
                .map(|(mut a, z)| {
                    a.extend(z);
                    a
                })
                .collect();
        }
        let d = seq[0].len();
        seq.iter()
            .map(|v| {
                let mut o: Vec<f32> = (0..self.classes)
                    .map(|c| {
                        self.fc_b[c]
                            + self.fc_w[c * d..(c + 1) * d]
                                .iter()
                                .zip(v)
                                .map(|(a, b)| a * b)
                                .sum::<f32>()
                    })
                    .collect();
                let m = o.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                let lse = m + o.iter().map(|v| (v - m).exp()).sum::<f32>().ln();
                o.iter_mut().for_each(|v| *v -= lse);
                o
            })
            .collect()
    }
}

/// The time (s from x[0]) of model frame j.
pub fn frame_time(j: usize) -> f64 {
    // feature frame i is centred on sample 2 i + 8; model frame j ~ 4 j + 1.5
    (8.0 * j as f64 + 11.0) / ENV_FS
}

/// Decode a channel whose carrier is within ±150 Hz of 0: centred first
/// (the network was trained with the carrier within ±10 Hz). Greedy CTC:
/// (time s, character) from x's start `t0`, spaces included.
pub fn decode(x: &[Complex32], t0: f64) -> Vec<(f64, String)> {
    let (f, _) = crate::decode::fine_freq(x, 0.0, 150.0);
    let w = -2.0 * std::f64::consts::PI * f / ENV_FS;
    let y: Vec<Complex32> = x
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let a = w * i as f64;
            v * Complex32::new(a.cos() as f32, a.sin() as f32)
        })
        .collect();
    decode_centred(&y, t0)
}

/// [`decode`] of a channel already centred.
pub fn decode_centred(x: &[Complex32], t0: f64) -> Vec<(f64, String)> {
    let lp = CwNet::get().forward(x);
    let mut out: Vec<(f64, String)> = Vec::new();
    let mut prev = 0;
    for (j, row) in lp.iter().enumerate() {
        let c = row
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|v| v.0)
            .unwrap_or(0);
        if c != prev && c != 0 {
            let s = CLASSES[c];
            if !(s == " " && out.last().is_none_or(|l| l.1 == " ")) {
                out.push((t0 + frame_time(j), s.to_string()));
            }
        }
        prev = c;
    }
    while out.last().is_some_and(|l| l.1 == " ") {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f16() {
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f16_to_f32(0xc000), -2.0);
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert!((f16_to_f32(0x3555) - 0.333_25).abs() < 1e-4);
        assert!((f16_to_f32(0x0001) - 5.96e-8).abs() < 1e-9);
    }

    /// The network as PyTorch computes it (tools/ml/cw/export.py).
    #[test]
    fn matches_pytorch() {
        let b: &[u8] = include_bytes!("../model/cwnet-ref.bin");
        assert_eq!(&b[..8], b"W8CWREF1");
        let rd = |p: usize| u32::from_le_bytes(b[p..p + 4].try_into().unwrap()) as usize;
        let f = |p: usize| f32::from_le_bytes(b[p..p + 4].try_into().unwrap());
        let n = rd(8);
        let x: Vec<Complex32> = (0..n)
            .map(|i| Complex32::new(f(12 + 8 * i), f(16 + 8 * i)))
            .collect();
        let p = 12 + 8 * n;
        let (t, c) = (rd(p), rd(p + 4));
        let lp = CwNet::get().forward(&x);
        assert_eq!((lp.len(), lp[0].len()), (t, c));
        let mut worst = 0f32;
        for j in 0..t {
            for k in 0..c {
                worst = worst.max((lp[j][k] - f(p + 8 + 4 * (j * c + k))).abs());
            }
        }
        assert!(worst < 2e-3, "max difference {worst}");
    }
}
