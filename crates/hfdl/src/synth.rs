// SPDX-License-Identifier: GPL-3.0-or-later
//! Test bursts: octets through the code, interleaver, scrambler and
//! preamble to symbols, then to audio (USB, centre at 1440 Hz) or IQ with
//! noise and a second (delayed) path.

use crate::front::rrc;
use crate::phy::{self, Mode};
use num_complex::Complex32;
use std::f64::consts::PI;

/// A tiny deterministic generator (xorshift).
pub struct Rng(pub u64);

impl Rng {
    pub fn bits(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn uniform(&mut self) -> f64 {
        (self.bits() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn gauss(&mut self) -> f64 {
        let (u, v) = (self.uniform().max(1e-300), self.uniform());
        (-2.0 * u.ln()).sqrt() * (2.0 * PI * v).cos()
    }
}

fn bpsk(b: u8) -> Complex32 {
    Complex32::new(if b == 0 { 1.0 } else { -1.0 }, 0.0)
}

/// A burst's symbols carrying `octets` (zero padded to the mode's size).
pub fn symbols(mode: Mode, octets: &[u8], rng: &mut Rng) -> Vec<Complex32> {
    let mut bits: Vec<u8> = octets
        .iter()
        .flat_map(|&o| (0..8).map(move |i| (o >> i) & 1))
        .collect();
    bits.resize(mode.user_bits(), 0);
    let mut coded = phy::encode(&bits);
    if mode.repeat {
        coded = coded.iter().flat_map(|&c| [c, c]).collect();
    }
    let perm = phy::interleave(&mode);
    let mut tx = vec![0u8; coded.len()];
    for (i, &p) in perm.iter().enumerate() {
        tx[p as usize] = coded[i];
    }
    let scr = phy::scrambler();
    let npt = 1 << mode.bits;
    let data: Vec<Complex32> = tx
        .chunks(mode.bits)
        .enumerate()
        .map(|(i, c)| {
            let v = c.iter().fold(0usize, |a, &b| (a << 1) | b as usize);
            let g = (0..mode.bits).fold(v, |a, s| a ^ (v >> (s + 1)));
            let a = 2.0 * PI * g as f64 / npt as f64 + if scr[i % 120] == 1 { PI } else { 0.0 };
            Complex32::new(a.cos() as f32, a.sin() as f32)
        })
        .collect();
    let mut s: Vec<Complex32> = (0..phy::PREKEY)
        .map(|_| bpsk((rng.bits() & 1) as u8))
        .collect();
    let a = phy::a();
    s.extend(a.iter().chain(&a).map(|&b| bpsk(b)));
    let m1 = mode.m1();
    s.extend(m1.iter().chain(&m1[..phy::M2_LEN]).map(|&b| bpsk(b)));
    for _ in 0..phy::T_PREAMBLE {
        s.extend(phy::T.iter().map(|&b| bpsk(b)));
    }
    for f in data.chunks(phy::DATA) {
        s.extend_from_slice(f);
        s.extend(phy::T.iter().map(|&b| bpsk(b)));
    }
    s
}

/// Channel: a frequency offset (Hz), and a second path (delay s, gain
/// relative to the first).
#[derive(Clone, Copy, Default)]
pub struct Channel {
    pub offset: f64,
    pub delay: f64,
    pub gain: f64,
}

/// Complex baseband at `rate`, the burst centred at `centre` Hz, starting
/// after `lead` s of silence.
pub fn iq(sym: &[Complex32], rate: f64, centre: f64, ch: Channel, lead: f64) -> Vec<Complex32> {
    let span = 8.0;
    let dur = sym.len() as f64 / phy::BAUD + 2.0 * lead;
    let n = (dur * rate) as usize;
    let mut out = vec![Complex32::new(0.0, 0.0); n];
    let paths = [(0.0, 1.0), (ch.delay, ch.gain)];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f64 / rate - lead;
        let mut v = Complex32::new(0.0, 0.0);
        for (d, g) in paths {
            if g == 0.0 {
                continue;
            }
            let u = (t - d) * phy::BAUD;
            let k0 = ((u - span).ceil().max(0.0)) as usize;
            let k1 = ((u + span).floor() as isize).min(sym.len() as isize - 1);
            for k in k0 as isize..=k1 {
                v += sym[k as usize] * (g * rrc(u - k as f64, 0.3)) as f32;
            }
        }
        let ph = 2.0 * PI * (centre + ch.offset) * t;
        *o = v * Complex32::new(ph.cos() as f32, ph.sin() as f32);
    }
    out
}

/// Adds noise for an SNR (dB) in 3 kHz, the signal's power measured over
/// `active` samples.
pub fn noise(x: &mut [Complex32], rate: f64, snr_db: f64, active: usize, rng: &mut Rng) {
    let p: f64 = x.iter().map(|v| v.norm_sqr() as f64).sum::<f64>() / active as f64;
    let n3k = p / 10f64.powf(snr_db / 10.0);
    let sigma = (n3k * rate / 3000.0 / 2.0).sqrt();
    for v in x {
        *v += Complex32::new((sigma * rng.gauss()) as f32, (sigma * rng.gauss()) as f32);
    }
}

/// A squitter's octets (66, the CRC right) for tests.
pub fn squitter(gs: u8, frame: u16) -> Vec<u8> {
    let mut b = vec![0u8; 66];
    b[0] = 0x20;
    b[1] = 0x80 | gs;
    b[2] = frame as u8;
    b[3] = (frame >> 8) as u8 & 0xf;
    b[53] = 52;
    b[54] = 0x30;
    let c = phy::crc16(&b[..64]);
    b[64..].copy_from_slice(&c.to_le_bytes());
    b
}
