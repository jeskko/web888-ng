// SPDX-License-Identifier: GPL-3.0-or-later
//! Neural noise reduction for SSB voice, CW and AM broadcast (M8, `doc/m8-browser-decoders.md`
//! §5.4): a small network trained in `tools/ml/nr` gives a gain in 0..1
//! for every bin of a short-time spectrum; the audio is that spectrum
//! times the gains. Mask only: it takes sound away and never adds any.
//!
//! Streaming at 12 kS/s: 256-point frames every 128 samples (square-root
//! periodic Hann, analysis and synthesis), 255 samples (21 ms) of
//! latency: an output hop needs the frame after it.
//! Features: log10 of each bin's power minus a running level (2 s
//! exponential mean of the frame's mean log power, started at the first
//! frame), / 2, clamped to [-6, 6]; and, if the network takes them, the
//! pitch features (the normalised autocorrelation of the last 512
//! samples at 24 lags of 30-200 samples). Network: a causal convolution
//! over 3 frames (ReLU), two GRU layers, a linear layer and a sigmoid,
//! to gains per bin or per band (then interpolated to the bins by the
//! model's own matrix). Must match `tools/ml/nr/common.py`.

mod restore;
pub use restore::{GenNet, Restorer};

use num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use std::collections::HashMap;
use std::sync::Arc;

pub const FS: f32 = 12000.0;
pub const NFFT: usize = 256;
pub const HOP: usize = 128;
pub const BINS: usize = NFFT / 2 + 1;
const LEVEL_S: f32 = 2.0;
/// The stream's delay (samples).
pub const LATENCY: usize = 2 * HOP - 1;

/// The shipped weights (f16).
pub static MODEL: &[u8] = include_bytes!("../model/nrnet.bin");
/// The built-in CW model (for copying by ear: keeps every keyed tone,
/// takes the noise between and around them).
pub static MODEL_CW: &[u8] = include_bytes!("../model/nrnet-cw.bin");
/// The built-in AM broadcast model (speech and music, after the AM
/// detector and the notch).
pub static MODEL_AM: &[u8] = include_bytes!("../model/nrnet-am.bin");

fn f16_to_f32(h: u16) -> f32 {
    let s = ((h >> 15) & 1) as u32;
    let e = ((h >> 10) & 0x1f) as u32;
    let m = (h & 0x3ff) as u32;
    let bits = if e == 0 {
        if m == 0 {
            s << 31
        } else {
            let mut e2: i32 = 127 - 15 + 1;
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

/// Tensors of an nrnet.bin by name: b"W8NRNET1", u32 count, then per
/// tensor u16 name length, name, u8 dims, u32 each dim, f16 values.
fn parse(b: &[u8]) -> Option<HashMap<String, Tensor>> {
    if b.get(..8)? != b"W8NRNET1" {
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

fn sigmoid(v: f32) -> f32 {
    1.0 / (1.0 + (-v).exp())
}

struct Gru {
    wih: Vec<f32>, // (3H, in)
    whh: Vec<f32>, // (3H, H)
    bih: Vec<f32>,
    bhh: Vec<f32>,
    inp: usize,
    h: usize,
}

impl Gru {
    /// One step: the state `hs` updated from input `x`.
    fn step(&self, x: &[f32], hs: &mut [f32], gi: &mut [f32], gh: &mut [f32]) {
        let h = self.h;
        for r in 0..3 * h {
            gi[r] = self.bih[r] + dot(&self.wih[r * self.inp..(r + 1) * self.inp], x);
            gh[r] = self.bhh[r] + dot(&self.whh[r * h..(r + 1) * h], hs);
        }
        for j in 0..h {
            let r = sigmoid(gi[j] + gh[j]);
            let z = sigmoid(gi[h + j] + gh[h + j]);
            let n = (gi[2 * h + j] + r * gh[2 * h + j]).tanh();
            hs[j] = (1.0 - z) * n + z * hs[j];
        }
    }
}

/// The pitch feature's lags (samples; 400-60 Hz), as `PITCH_LAGS` in
/// common.py.
pub const PITCH_LAGS: [usize; 24] = [
    30, 33, 35, 38, 42, 45, 49, 53, 58, 63, 68, 74, 81, 88, 95, 103, 112, 122, 132, 144, 156, 170,
    184, 200,
];
/// The pitch window (samples).
pub const PITCH_N: usize = 512;

/// The network's weights.
pub struct Net {
    /// Convolution (H, nin, 3) rearranged as (H, 3 × nin): taps for
    /// frames t-2, t-1, t one after another.
    conv_w: Vec<f32>,
    conv_b: Vec<f32>,
    gru: [Gru; 2],
    out_w: Vec<f32>, // (nout, H)
    out_b: Vec<f32>,
    /// Gains per band to gains per bin (nout = bands, BINS), if the
    /// network gives bands.
    band: Option<Vec<f32>>,
    h: usize,
    /// Its input: BINS features, then the pitch features if any.
    nin: usize,
    nout: usize,
}

impl Net {
    pub fn load(bytes: &[u8]) -> Option<Net> {
        let mut t = parse(bytes)?;
        let mut take = |k: &str| t.remove(k);
        let (cs, cw) = take("conv.weight")?;
        let (h, nin, k) = (cs[0], cs[1], cs[2]);
        if (nin != BINS && nin != BINS + PITCH_LAGS.len()) || k != 3 {
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
        let band = take("band");
        let mut gru = |l: usize, inp: usize| -> Option<Gru> {
            Some(Gru {
                wih: take(&format!("gru.weight_ih_l{l}"))?.1,
                whh: take(&format!("gru.weight_hh_l{l}"))?.1,
                bih: take(&format!("gru.bias_ih_l{l}"))?.1,
                bhh: take(&format!("gru.bias_hh_l{l}"))?.1,
                inp,
                h,
            })
        };
        let gru = [gru(0, h)?, gru(1, h)?];
        let out_w = take("out.weight")?.1;
        let out_b = take("out.bias")?.1;
        let nout = out_b.len();
        let ok = out_w.len() == nout * h
            && match &band {
                Some((s, _)) => s[..] == [nout, BINS],
                None => nout == BINS,
            };
        ok.then_some(Net {
            conv_w,
            conv_b,
            gru,
            out_w,
            out_b,
            band: band.map(|b| b.1),
            h,
            nin,
            nout,
        })
    }

    /// The shipped model.
    pub fn get() -> &'static Net {
        static NET: std::sync::OnceLock<Net> = std::sync::OnceLock::new();
        NET.get_or_init(|| Net::load(MODEL).expect("nrnet.bin"))
    }

    /// The built-in CW model.
    pub fn get_cw() -> &'static Net {
        static NET: std::sync::OnceLock<Net> = std::sync::OnceLock::new();
        NET.get_or_init(|| Net::load(MODEL_CW).expect("nrnet-cw.bin"))
    }

    /// The built-in AM broadcast model.
    pub fn get_am() -> &'static Net {
        static NET: std::sync::OnceLock<Net> = std::sync::OnceLock::new();
        NET.get_or_init(|| Net::load(MODEL_AM).expect("nrnet-am.bin"))
    }
}

/// A stream through the network: audio in, the same amount out,
/// [`LATENCY`] samples later.
pub struct Denoiser {
    net: &'static Net,
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
    win: Vec<f32>,
    pfft: Arc<dyn Fft<f32>>,
    pifft: Arc<dyn Fft<f32>>,
    pwin: Vec<f32>,
    pbuf: Vec<Complex32>,
    bgain: Vec<f32>,
    alpha: f32,
    /// 0..1: the share of what the network takes away that is kept
    /// (0: all of it goes).
    pub floor: f32,
    /// At least 1: the network's gain raised to this before the floor (over 1:
    /// more of what it partly takes away goes; what it keeps stays).
    pub power: f32,
    /// The generative restorer, if any (it needs a mask model with the
    /// pitch features), and its share of the output (0..1, the rest the
    /// mask's).
    restore: Option<Restorer>,
    pub mix: f32,
    ebuf: Vec<Complex32>,
    inp: Vec<f32>,      // the last PITCH_N input samples
    fill: usize,        // new samples in the current hop
    level: Option<f32>, // the running level
    feats: [Vec<f32>; 3],
    h: [Vec<f32>; 2],
    tail: Vec<f32>, // the last frame's second half, to add
    // scratch
    buf: Vec<Complex32>,
    conv: Vec<f32>,
    gi: Vec<f32>,
    gh: Vec<f32>,
    gain: Vec<f32>,
    out: Vec<f32>,
}

impl Denoiser {
    pub fn new(net: &'static Net) -> Self {
        let mut p = FftPlanner::new();
        let h = net.h;
        Denoiser {
            net,
            fft: p.plan_fft_forward(NFFT),
            ifft: p.plan_fft_inverse(NFFT),
            win: (0..NFFT)
                .map(|i| {
                    (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / NFFT as f64).cos()).sqrt()
                        as f32
                })
                .collect(),
            pfft: p.plan_fft_forward(2 * PITCH_N),
            pifft: p.plan_fft_inverse(2 * PITCH_N),
            pwin: (0..PITCH_N)
                .map(|i| {
                    (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / PITCH_N as f64).cos())
                        as f32
                })
                .collect(),
            pbuf: vec![Complex32::new(0.0, 0.0); 2 * PITCH_N],
            bgain: vec![0.0; net.nout],
            alpha: 1.0 - (-(HOP as f32) / FS / LEVEL_S).exp(),
            floor: 0.0,
            power: 1.0,
            restore: None,
            mix: 1.0,
            ebuf: vec![Complex32::new(0.0, 0.0); BINS],
            inp: vec![0.0; PITCH_N],
            fill: 0,
            level: None,
            feats: [vec![0.0; net.nin], vec![0.0; net.nin], vec![0.0; net.nin]],
            h: [vec![0.0; h], vec![0.0; h]],
            tail: vec![0.0; HOP],
            buf: vec![Complex32::new(0.0, 0.0); NFFT],
            conv: vec![0.0; h],
            gi: vec![0.0; 3 * h],
            gh: vec![0.0; 3 * h],
            gain: vec![0.0; BINS],
            out: Vec::new(),
        }
    }

    /// Back to the start (a new station, a gap).
    pub fn reset(&mut self) {
        let (floor, power, mix) = (self.floor, self.power, self.mix);
        let mut restore = self.restore.take();
        *self = Denoiser::new(self.net);
        self.floor = floor;
        self.power = power;
        self.mix = mix;
        if let Some(r) = restore.as_mut() {
            r.reset();
        }
        self.restore = restore;
    }

    /// Restore with `r` from the next frame on (None: the mask alone).
    /// False (and nothing changed) if the mask model has no pitch
    /// features.
    pub fn set_restorer(&mut self, r: Option<Restorer>) -> bool {
        if r.is_some() && self.net.nin == BINS {
            return false;
        }
        self.restore = r;
        true
    }

    pub fn restorer_mut(&mut self) -> Option<&mut Restorer> {
        self.restore.as_mut()
    }

    /// Feed audio; returns as many samples as were fed, [`LATENCY`]
    /// late (silence at first).
    pub fn process(&mut self, x: &[f32]) -> Vec<f32> {
        let mut y = Vec::with_capacity(x.len());
        for &v in x {
            self.inp[PITCH_N - HOP + self.fill] = v;
            self.fill += 1;
            if self.fill == HOP {
                self.frame();
                self.fill = 0;
                self.inp.copy_within(HOP.., 0);
            }
            // the output hop made last, sample by sample
            y.push(self.out.get(self.fill).copied().unwrap_or(0.0));
        }
        y
    }

    /// One frame: the last NFFT inputs → the next HOP outputs.
    fn frame(&mut self) {
        let net = self.net;
        let last = &self.inp[PITCH_N - NFFT..];
        for (b, (&v, &w)) in self.buf.iter_mut().zip(last.iter().zip(&self.win)) {
            *b = Complex32::new(v * w, 0.0);
        }
        self.fft.process(&mut self.buf);
        // features
        self.feats.rotate_left(1);
        let f = &mut self.feats[2];
        let mut mean = 0.0;
        for (k, fk) in f[..BINS].iter_mut().enumerate() {
            *fk = (self.buf[k].norm_sqr() + 1e-10).log10();
            mean += *fk;
        }
        mean /= BINS as f32;
        let lev = match self.level {
            None => mean,
            Some(m) => m + self.alpha * (mean - m),
        };
        self.level = Some(lev);
        for fk in f[..BINS].iter_mut() {
            *fk = ((*fk - lev) / 2.0).clamp(-6.0, 6.0);
        }
        if net.nin > BINS {
            // pitch: the normalised autocorrelation of the last PITCH_N
            // samples (Hann), at PITCH_LAGS, times 2
            for (i, b) in self.pbuf.iter_mut().enumerate() {
                *b = if i < PITCH_N {
                    Complex32::new(self.inp[i] * self.pwin[i], 0.0)
                } else {
                    Complex32::new(0.0, 0.0)
                };
            }
            self.pfft.process(&mut self.pbuf);
            for b in self.pbuf.iter_mut() {
                *b = Complex32::new(b.norm_sqr(), 0.0);
            }
            self.pifft.process(&mut self.pbuf);
            let s = 1.0 / (2 * PITCH_N) as f32;
            let r0 = self.pbuf[0].re * s;
            for (j, &l) in PITCH_LAGS.iter().enumerate() {
                f[BINS + j] = 2.0 * self.pbuf[l].re * s / (r0 + 1e-9);
            }
        }
        // network
        let n = net.nin;
        for (o, c) in self.conv.iter_mut().enumerate() {
            let w = &net.conv_w[o * 3 * n..(o + 1) * 3 * n];
            let s = net.conv_b[o]
                + dot(&w[..n], &self.feats[0])
                + dot(&w[n..2 * n], &self.feats[1])
                + dot(&w[2 * n..], &self.feats[2]);
            *c = s.max(0.0);
        }
        let [h0, h1] = &mut self.h;
        net.gru[0].step(&self.conv, h0, &mut self.gi, &mut self.gh);
        net.gru[1].step(h0, h1, &mut self.gi, &mut self.gh);
        for (k, g) in self.bgain.iter_mut().enumerate() {
            *g = sigmoid(net.out_b[k] + dot(&net.out_w[k * net.h..(k + 1) * net.h], h1));
        }
        match &net.band {
            Some(m) => {
                self.gain.fill(0.0);
                for (j, &gb) in self.bgain.iter().enumerate() {
                    for (g, &w) in self.gain.iter_mut().zip(&m[j * BINS..(j + 1) * BINS]) {
                        *g += gb * w;
                    }
                }
            }
            None => self.gain.copy_from_slice(&self.bgain),
        }
        if let Some(r) = self.restore.as_mut() {
            // the restorer starts from the mask's own output (no floor,
            // no power), as in training
            for k in 0..BINS {
                self.ebuf[k] = self.buf[k] * self.gain[k];
            }
            let f = &self.feats[2];
            r.frame(&f[..BINS], &f[BINS..], lev, &mut self.ebuf);
        }
        if self.power != 1.0 {
            for g in self.gain.iter_mut() {
                *g = g.max(0.0).powf(self.power);
            }
        }
        for g in self.gain.iter_mut() {
            *g = self.floor + (1.0 - self.floor) * *g;
        }
        // the masked spectrum back (Hermitian: the upper half mirrors)
        for k in 0..BINS {
            self.buf[k] *= self.gain[k];
        }
        if self.restore.is_some() {
            let m = self.mix.clamp(0.0, 1.0);
            for k in 0..BINS {
                self.buf[k] = self.buf[k] * (1.0 - m) + self.ebuf[k] * m;
            }
        }
        for k in 1..NFFT / 2 {
            self.buf[NFFT - k] = self.buf[k].conj();
        }
        self.ifft.process(&mut self.buf);
        let s = 1.0 / NFFT as f32;
        self.out.clear();
        for i in 0..HOP {
            self.out
                .push(self.tail[i] + self.buf[i].re * s * self.win[i]);
        }
        for i in 0..HOP {
            self.tail[i] = self.buf[HOP + i].re * s * self.win[HOP + i];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The references from export.py: input audio and the PyTorch output
    /// (f16 weights, floor 0).
    #[test]
    fn matches_pytorch() {
        check(Net::get(), include_bytes!("../model/nrnet-ref.bin"));
    }

    #[test]
    fn cw_matches_pytorch() {
        check(Net::get_cw(), include_bytes!("../model/nrnet-cw-ref.bin"));
    }

    #[test]
    fn am_matches_pytorch() {
        check(Net::get_am(), include_bytes!("../model/nrnet-am-ref.bin"));
    }

    /// The restorer on the shipped mask model (export.py --gen: no start
    /// noise, 2 steps, all of its output).
    #[test]
    fn restorer_matches_pytorch() {
        let mut r = Restorer::new(GenNet::load(include_bytes!("../model/nrgen.bin")).unwrap());
        r.sigma = 0.0;
        r.steps = 2;
        check_with(
            Net::get(),
            include_bytes!("../model/nrgen-ref.bin"),
            Some(r),
        );
    }

    fn check(net: &'static Net, b: &[u8]) {
        check_with(net, b, None)
    }

    fn check_with(net: &'static Net, b: &[u8], r: Option<Restorer>) {
        assert_eq!(&b[..8], b"W8NRREF1");
        let n = u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize;
        let f = |o: usize| -> Vec<f32> {
            b[o..o + 4 * n]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c))
                .collect()
        };
        let (x, want) = (f(12), f(12 + 4 * n));
        let mut d = Denoiser::new(net);
        if r.is_some() {
            assert!(d.set_restorer(r));
        }
        // odd chunks, as the page's blocks come
        let mut y = Vec::new();
        for c in x.chunks(333) {
            y.extend(d.process(c));
        }
        let peak = want.iter().fold(0f32, |m, v| m.max(v.abs()));
        let err = (0..n - LATENCY)
            .map(|i| (y[i + LATENCY] - want[i]).abs())
            .fold(0f32, f32::max);
        assert!(err < 2e-3 * peak, "max error {err} (peak {peak})");
    }
}
