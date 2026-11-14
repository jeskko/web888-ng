// SPDX-License-Identifier: GPL-3.0-or-later
//! FSK synthesis for tests: async characters, SITOR-B FEC streams,
//! phase-continuous two-tone FSK with noise and (selective) fading.

use crate::analyze::Parity;
use crate::charset::{self, C476_ALPHA, C476_REP};
use num_complex::Complex32;
use web888_cw::dsp::Rng;

/// (level, duration in bits).
pub type Seg = Vec<(u8, f64)>;

/// Start, data LSB first, parity, stop; `idle` extra mark bits after
/// each character; `lead` and `tail` bits of mark.
pub fn async_bits(
    codes: &[u32],
    ndata: u32,
    parity: Parity,
    stop: f64,
    idle: &mut dyn FnMut() -> f64,
    lead: f64,
    tail: f64,
) -> Seg {
    let mut seg = vec![(1, lead)];
    for &c in codes {
        let mut b: Vec<u8> = (0..ndata).map(|i| ((c >> i) & 1) as u8).collect();
        let ones = b.iter().map(|&v| v as u32).sum::<u32>();
        match parity {
            Parity::Even => b.push((ones & 1) as u8),
            Parity::Odd => b.push(((ones + 1) & 1) as u8),
            Parity::None => {}
        }
        seg.push((0, 1.0));
        seg.extend(b.iter().map(|&v| (v, 1.0)));
        seg.push((1, stop + idle()));
    }
    seg.push((1, tail));
    seg
}

/// The usual: no idle, 4 bits of mark before and after.
pub fn async_simple(codes: &[u32], ndata: u32, parity: Parity, stop: f64) -> Seg {
    async_bits(codes, ndata, parity, stop, &mut || 0.0, 4.0, 4.0)
}

/// SITOR-B FEC bit stream: DX/RX slots, RX = DX delayed 5 slots.
pub fn sitor_bits(text: &str, phasing: usize) -> Seg {
    let msg = charset::ccir476_encode(text);
    let mut dx = vec![C476_ALPHA; phasing];
    dx.extend(&msg);
    dx.extend([C476_ALPHA; 4]);
    let mut seg = Vec::new();
    for (j, &c) in dx.iter().enumerate() {
        let k = j as isize - phasing as isize - 2;
        let rx = if k >= 0 && (k as usize) < msg.len() {
            msg[k as usize]
        } else {
            C476_REP
        };
        for s in [c, rx] {
            for i in 0..7 {
                seg.push((((s >> i) & 1) as u8, 1.0));
            }
        }
    }
    seg
}

/// Signal conditions.
#[derive(Clone, Debug)]
pub struct Cond {
    /// Centre between the tones (Hz).
    pub f0: f64,
    /// Signal over noise in 2500 Hz (dB).
    pub snr_db: Option<f64>,
    pub mark_high: bool,
    pub fade_hz: f64,
    /// Independent fading per tone.
    pub selective: bool,
}

impl Cond {
    pub fn new(f0: f64) -> Self {
        Cond {
            f0,
            snr_db: None,
            mark_high: true,
            fade_hz: 0.0,
            selective: false,
        }
    }
}

struct Fading {
    hz: f64,
    p: Vec<(f64, f64)>,
}

impl Fading {
    fn new(hz: f64, rng: &mut Rng) -> Self {
        let tp = 2.0 * std::f64::consts::PI;
        Fading {
            hz,
            p: (0..16)
                .map(|_| ((tp * rng.uniform()).cos(), tp * rng.uniform()))
                .collect(),
        }
    }

    fn gain(&self, t: f64) -> Complex32 {
        let (mut re, mut im) = (0.0, 0.0);
        for (c, ph) in &self.p {
            let a = 2.0 * std::f64::consts::PI * self.hz * c * t + ph;
            re += a.cos();
            im += a.sin();
        }
        Complex32::new((re / 4.0) as f32, (im / 4.0) as f32)
    }
}

/// Levels per sample at fs.
fn levels(seg: &[(u8, f64)], baud: f64, fs: f64) -> Vec<u8> {
    let total: f64 = seg.iter().map(|s| s.1).sum();
    let n = (total / baud * fs) as usize;
    let mut out = Vec::with_capacity(n);
    let mut edge = 0.0;
    let mut k = 0;
    for i in 0..n {
        let t = i as f64 / fs * baud;
        while k + 1 < seg.len() && t >= edge + seg[k].1 {
            edge += seg[k].1;
            k += 1;
        }
        out.push(seg[k].0);
    }
    out
}

/// Phase-continuous complex FSK at fs (unit power).
pub fn complex(
    seg: &[(u8, f64)],
    baud: f64,
    shift: f64,
    fs: f64,
    c: &Cond,
    rng: &mut Rng,
) -> Vec<Complex32> {
    let lev = levels(seg, baud, fs);
    let mut ph = 2.0 * std::f64::consts::PI * rng.uniform();
    let fa = (c.fade_hz > 0.0).then(|| Fading::new(c.fade_hz, rng));
    let fb = (c.fade_hz > 0.0 && c.selective).then(|| Fading::new(c.fade_hz, rng));
    let sig = c
        .snr_db
        .map(|s| (10f64.powf(-s / 10.0) / 2500.0 * fs / 2.0).sqrt());
    lev.iter()
        .enumerate()
        .map(|(i, &l)| {
            let hi = if c.mark_high { l == 1 } else { l == 0 };
            let f = c.f0 + if hi { shift / 2.0 } else { -shift / 2.0 };
            ph += 2.0 * std::f64::consts::PI * f / fs;
            let mut v = Complex32::new(ph.cos() as f32, ph.sin() as f32);
            let t = i as f64 / fs;
            if let Some(g) = &fa {
                let g = match (&fb, hi) {
                    (Some(g2), false) => g2.gain(t),
                    _ => g.gain(t),
                };
                v *= g;
            }
            if let Some(s) = sig {
                v += Complex32::new((s * rng.normal()) as f32, (s * rng.normal()) as f32);
            }
            v
        })
        .collect()
}

/// Real audio FSK at fs: amplitude amp around audio frequency c.f0.
pub fn audio(
    seg: &[(u8, f64)],
    baud: f64,
    shift: f64,
    fs: f64,
    c: &Cond,
    amp: f64,
    rng: &mut Rng,
) -> Vec<f32> {
    let lev = levels(seg, baud, fs);
    let mut ph = 2.0 * std::f64::consts::PI * rng.uniform();
    lev.iter()
        .map(|&l| {
            let hi = if c.mark_high { l == 1 } else { l == 0 };
            ph += 2.0 * std::f64::consts::PI * (c.f0 + if hi { shift / 2.0 } else { -shift / 2.0 })
                / fs;
            (amp * ph.cos()) as f32
        })
        .collect()
}
