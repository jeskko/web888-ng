// SPDX-License-Identifier: GPL-3.0-or-later
//! The generative voice restorer ("restore, may invent"; `tools/ml/nr/gen.py`,
//! `doc/m8-browser-decoders.md` §5.4): a conditional flow from the mask
//! model's output to clean speech, frame by frame in the mask model's
//! stream (same STFT, same latency). Unlike the mask it can add sound.
//!
//! Per frame: the noisy spectrum's features, the mask output's (its log
//! power against the same running level) and the pitch features go
//! through a causal convolution and two GRU layers (once a frame); then
//! from x = c(E) + sigma·noise (c: the spectrum over the running level,
//! as an amplitude, its magnitude to the power 0.5) a head (an MLP of
//! two hidden layers, SiLU) gives the velocity, `steps` Euler steps; the
//! frame's spectrum is c⁻¹(x). Must match gen.py.
//!
//! Two forms, told apart by the weights (the head's output): the complex
//! spectrum (2 BINS values a frame, real and imaginary parts), or the
//! magnitude only (BINS; gen.py --mag), the mask output's phase kept
//! (the noisy input's: the mask's gains are real).

use crate::{BINS, Gru, PITCH_LAGS, dot, parse};
use num_complex::Complex32;

const NT: usize = 16;
const SIGMA: f32 = 0.2;

struct Linear {
    w: Vec<f32>, // (out, in)
    b: Vec<f32>,
    inp: usize,
}

impl Linear {
    fn apply(&self, x: &[f32], y: &mut [f32]) {
        for (o, v) in y.iter_mut().enumerate() {
            *v = self.b[o] + dot(&self.w[o * self.inp..(o + 1) * self.inp], x);
        }
    }
}

/// The restorer's weights (nrgen.bin: the nrnet.bin format).
pub struct GenNet {
    conv_w: Vec<f32>, // (H, 3 × nin)
    conv_b: Vec<f32>,
    gru: [Gru; 2],
    h1: Linear,
    h2: Linear,
    h3: Linear,
    h: usize,
    nin: usize,
    /// Values a frame: 2 BINS (complex) or BINS (magnitude only).
    d: usize,
}

impl GenNet {
    pub fn load(bytes: &[u8]) -> Option<GenNet> {
        let mut t = parse(bytes)?;
        let mut take = |k: &str| t.remove(k);
        let (cs, cw) = take("conv.weight")?;
        let (h, nin, k) = (cs[0], cs[1], cs[2]);
        if nin != 2 * BINS + PITCH_LAGS.len() || k != 3 {
            return None;
        }
        let mut conv_w = vec![0f32; h * 3 * nin];
        for o in 0..h {
            for c in 0..nin {
                for kk in 0..3 {
                    conv_w[o * 3 * nin + kk * nin + c] = cw[(o * nin + c) * 3 + kk];
                }
            }
        }
        let conv_b = take("conv.bias")?.1;
        let mut gru = |l: usize| -> Option<Gru> {
            Some(Gru {
                wih: take(&format!("gru.weight_ih_l{l}"))?.1,
                whh: take(&format!("gru.weight_hh_l{l}"))?.1,
                bih: take(&format!("gru.bias_ih_l{l}"))?.1,
                bhh: take(&format!("gru.bias_hh_l{l}"))?.1,
                inp: h,
                h,
            })
        };
        let gru = [gru(0)?, gru(1)?];
        let mut lin = |n: &str| -> Option<Linear> {
            let (s, w) = take(&format!("{n}.weight"))?;
            Some(Linear {
                w,
                b: take(&format!("{n}.bias"))?.1,
                inp: s[1],
            })
        };
        let (h1, h2, h3) = (lin("h1")?, lin("h2")?, lin("h3")?);
        let d = h3.b.len();
        let ok = (d == BINS || d == 2 * BINS)
            && h1.inp == h + 2 * d + 2 * NT
            && h2.inp == h1.b.len()
            && h3.inp == h2.b.len();
        ok.then_some(GenNet {
            conv_w,
            conv_b,
            gru,
            h1,
            h2,
            h3,
            h,
            nin,
            d,
        })
    }

    /// The magnitude-only form.
    pub fn mag(&self) -> bool {
        self.d == BINS
    }
}

/// The restorer's state in a stream.
pub struct Restorer {
    net: GenNet,
    /// Euler steps (1-4).
    pub steps: usize,
    /// The start's noise (0: deterministic, for the test).
    pub sigma: f32,
    rng: u64,
    feats: [Vec<f32>; 3],
    hs: [Vec<f32>; 2],
    conv: Vec<f32>,
    gi: Vec<f32>,
    gh: Vec<f32>,
    z: Vec<f32>, // the head's input
    a: Vec<f32>,
    b: Vec<f32>,
    v: Vec<f32>,
}

impl Restorer {
    pub fn new(net: GenNet) -> Restorer {
        let (h, nin) = (net.h, net.nin);
        let (w1, w2, d) = (net.h1.b.len(), net.h2.b.len(), net.d);
        Restorer {
            steps: 2,
            sigma: SIGMA,
            rng: 0x9e37_79b9_7f4a_7c15,
            feats: [vec![0.0; nin], vec![0.0; nin], vec![0.0; nin]],
            hs: [vec![0.0; h], vec![0.0; h]],
            conv: vec![0.0; h],
            gi: vec![0.0; 3 * h],
            gh: vec![0.0; 3 * h],
            z: vec![0.0; h + 2 * d + 2 * NT],
            a: vec![0.0; w1],
            b: vec![0.0; w2],
            v: vec![0.0; d],
            net,
        }
    }

    /// Back to the start (the weights kept).
    pub fn reset(&mut self) {
        let h = self.net.h;
        for f in self.feats.iter_mut() {
            f.fill(0.0);
        }
        self.hs = [vec![0.0; h], vec![0.0; h]];
    }

    fn gauss(&mut self) -> f32 {
        // xorshift64* and Box-Muller (one of the pair)
        let mut u = || {
            self.rng ^= self.rng >> 12;
            self.rng ^= self.rng << 25;
            self.rng ^= self.rng >> 27;
            ((self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as f32 + 0.5)
                / (1u64 << 24) as f32
        };
        let (a, b) = (u(), u());
        (-2.0 * a.ln()).sqrt() * (std::f32::consts::TAU * b).cos()
    }

    /// One frame: the noisy spectrum's features `fx` (BINS) and pitch
    /// features, the running level `lev` (log10 power), the mask output
    /// `e` (BINS) → the restored spectrum in `e`.
    pub(crate) fn frame(&mut self, fx: &[f32], pitch: &[f32], lev: f32, e: &mut [Complex32]) {
        let net = &self.net;
        let n = net.nin;
        self.feats.rotate_left(1);
        let f = &mut self.feats[2];
        f[..BINS].copy_from_slice(fx);
        for (fk, ek) in f[BINS..2 * BINS].iter_mut().zip(e.iter()) {
            *fk = (((ek.norm_sqr() + 1e-10).log10() - lev) / 2.0).clamp(-6.0, 6.0);
        }
        f[2 * BINS..].copy_from_slice(pitch);
        for (o, c) in self.conv.iter_mut().enumerate() {
            let w = &net.conv_w[o * 3 * n..(o + 1) * 3 * n];
            let s = net.conv_b[o]
                + dot(&w[..n], &self.feats[0])
                + dot(&w[n..2 * n], &self.feats[1])
                + dot(&w[2 * n..], &self.feats[2]);
            *c = s.max(0.0);
        }
        let [h0, h1] = &mut self.hs;
        net.gru[0].step(&self.conv, h0, &mut self.gi, &mut self.gh);
        net.gru[1].step(h0, h1, &mut self.gi, &mut self.gh);
        // c(E) and the start
        let (h, d, mag) = (net.h, net.d, net.mag());
        let scale = 10f32.powf(lev / 2.0);
        self.z[..h].copy_from_slice(&self.hs[1]);
        let (ce, rest) = self.z[h..].split_at_mut(d);
        for (k, ek) in e.iter().enumerate() {
            if mag {
                ce[k] = (ek.norm() / scale).max(1e-9).sqrt();
            } else {
                let zk = *ek / scale;
                let c = zk * zk.norm().max(1e-9).powf(-0.5);
                ce[k] = c.re;
                ce[BINS + k] = c.im;
            }
        }
        rest[..d].copy_from_slice(ce);
        if self.sigma > 0.0 {
            for k in 0..d {
                let g = self.gauss();
                self.z[h + d + k] += self.sigma * g;
            }
        }
        let steps = self.steps.clamp(1, 4);
        let x0 = h + d;
        let t0 = h + 2 * d;
        for s in 0..steps {
            let t = s as f32 / steps as f32;
            for i in 0..NT {
                let a = t * std::f32::consts::PI * (i + 1) as f32;
                self.z[t0 + i] = a.sin();
                self.z[t0 + NT + i] = a.cos();
            }
            let net = &self.net;
            net.h1.apply(&self.z, &mut self.a);
            for v in self.a.iter_mut() {
                *v *= sigmoid(*v);
            }
            net.h2.apply(&self.a, &mut self.b);
            for v in self.b.iter_mut() {
                *v *= sigmoid(*v);
            }
            net.h3.apply(&self.b, &mut self.v);
            for k in 0..d {
                self.z[x0 + k] += self.v[k] / steps as f32;
            }
        }
        // c⁻¹
        for (k, ek) in e.iter_mut().enumerate() {
            if mag {
                let m = self.z[x0 + k].max(0.0);
                *ek = *ek / ek.norm().max(1e-9) * (m * m * scale);
            } else {
                let zk = Complex32::new(self.z[x0 + k], self.z[x0 + BINS + k]);
                *ek = zk * zk.norm().max(1e-9) * scale;
            }
        }
    }
}

fn sigmoid(v: f32) -> f32 {
    1.0 / (1.0 + (-v).exp())
}
