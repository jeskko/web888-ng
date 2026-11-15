// SPDX-License-Identifier: GPL-3.0-or-later
//! Bursts from the matched filter's output (4 samples a symbol):
//!
//! * Detection: the two A sequences (254 symbols) correlated in 16 pieces
//!   of 16 symbols, the pieces' powers summed (a frequency offset turns
//!   the pieces, so this holds to ~±40 Hz) over the signal's energy: ~1/16
//!   for noise, near 1 for a clean burst. The best within 130 symbols of
//!   the first over [`Demod::DETECT`] is taken (half the A pair matches
//!   127 symbols early).
//! * Frequency: the pieces' phase steps (coarse), then a line fitted to
//!   their unwrapped phases.
//! * Equaliser: 25 taps at half a symbol (±6 symbols of multipath),
//!   fitted by least squares to the A pair, then followed through the
//!   burst by normalised LMS: on known symbols (M2, the 9 T sequences and
//!   each frame's T probe) and decisions (M1, data), with a phase loop.
//! * M1: which of the eight rotations matches the equalised symbols best
//!   (rate and slot length); none well enough ends the burst.
//! * Soft bits: max-log over the constellation, scaled by the noise the
//!   probes show; descrambled, deinterleaved, Viterbi decoded.

use crate::front::SPS;
use crate::phy::{self, Mode};
use num_complex::{Complex32, Complex64};

const PIECE: usize = 16;
const AA: usize = 2 * phy::A_LEN;
/// Equaliser taps each side of the centre (half symbols).
const EQ_HALF: usize = 8;
const EQ: usize = 2 * EQ_HALF + 1;
/// Samples before / after a symbol the equaliser reads.
const EQ_REACH: usize = EQ_HALF * SPS / 2;

/// A demodulated burst.
#[derive(Clone, Debug)]
pub struct Burst {
    /// Sample (at 4 a symbol) of the first A.
    pub start: u64,
    pub mode: Mode,
    /// Frequency offset (Hz).
    pub freq: f32,
    /// SNR from the probes (dB, in the symbol rate's bandwidth).
    pub snr: f32,
    /// Detection metric and M1's correlation.
    pub detect: f32,
    pub m1: f32,
    /// The decoded octets.
    pub octets: Vec<u8>,
    /// The data symbols (equalised, before descrambling) and the noise
    /// variance the probes show.
    pub symbols: Vec<Complex32>,
    pub sigma2: f32,
    /// The probes' gains (the preamble's last T, then each frame's): the
    /// channel's gain and phase as the equaliser leaves it.
    pub gains: Vec<Complex32>,
}

/// One burst being demodulated.
struct Rx {
    n0: u64,
    detect: f32,
    /// Frequency offset, cycles a sample.
    f: f64,
    /// Phase at n0 (the A pair's).
    w: [Complex32; EQ],
    k: usize,
    theta: f32,
    nu: f32,
    mode: Option<Mode>,
    m1: f32,
    z_m1: Vec<Complex32>,
    soft: Vec<Complex32>,
    /// z·d over the probes (the preamble's last T, then each frame's).
    probes: Vec<Complex32>,
}

/// The receiver's constants.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// LMS step on known symbols and on decisions.
    pub mu_known: f32,
    pub mu_dd: f32,
    /// Phase loop: proportional and integral gains.
    pub pll: f32,
    pub pll_i: f32,
    /// Least squares regularisation (of the mean diagonal).
    pub ridge: f64,
    /// The phase loop also follows decisions (not only known symbols).
    pub pll_dd: bool,
    /// Data scaled by the gain between the probes either side.
    pub interp: bool,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            mu_known: 0.03,
            mu_dd: 0.0,
            pll: 0.03,
            pll_i: 2e-4,
            ridge: 3e-2,
            pll_dd: false,
            interp: true,
        }
    }
}

pub struct Demod {
    pub params: Params,
    /// Matched filter output from absolute sample `base`.
    y: Vec<Complex32>,
    base: u64,
    next: u64,
    cand: Option<(u64, f32)>,
    cand_until: u64,
    busy_until: u64,
    rx: Option<Rx>,
    aa: [f32; AA],
    out: Vec<Burst>,
    /// Bursts started and ended without a mode (for statistics).
    pub false_starts: u32,
    /// Keep every detection metric over 0.12 (diagnostics).
    pub trace: bool,
    pub peaks: Vec<(u64, f32)>,
}

impl Default for Demod {
    fn default() -> Self {
        Self::new()
    }
}

fn bpsk(b: u8) -> f32 {
    if b == 0 { 1.0 } else { -1.0 }
}

impl Demod {
    /// Detection threshold (noise ~0.06).
    pub const DETECT: f32 = 0.18;
    /// M1 correlation needed.
    pub const M1: f32 = 0.4;

    pub fn new() -> Self {
        let a = phy::a();
        Demod {
            params: Params::default(),
            y: Vec::new(),
            base: 0,
            next: 0,
            cand: None,
            cand_until: 0,
            busy_until: 0,
            rx: None,
            aa: std::array::from_fn(|k| bpsk(a[k % phy::A_LEN])),
            out: Vec::new(),
            false_starts: 0,
            trace: false,
            peaks: Vec::new(),
        }
    }

    fn at(&self, n: u64) -> Complex32 {
        self.y[(n - self.base) as usize]
    }

    fn end(&self) -> u64 {
        self.base + self.y.len() as u64
    }

    /// Takes matched filter output; returns the bursts finished.
    pub fn push(&mut self, y: &[Complex32]) -> Vec<Burst> {
        self.y.extend_from_slice(y);
        loop {
            if let Some(rx) = self.rx.take() {
                match self.run(rx) {
                    Some(rx) => {
                        self.rx = Some(rx);
                        break;
                    }
                    None => continue,
                }
            }
            if !self.search() {
                break;
            }
        }
        // keep what a burst start may still need
        let keep = self
            .rx
            .as_ref()
            .map_or(self.next, |r| r.n0)
            .saturating_sub(EQ_REACH as u64 + 64)
            .min(self.next.saturating_sub(64));
        if keep > self.base + 65536 {
            let d = (keep - self.base) as usize;
            self.y.drain(..d);
            self.base += d as u64;
        }
        std::mem::take(&mut self.out)
    }

    /// The detection metric at n and the A pair's pieces.
    fn metric(&self, n: u64) -> (f32, [Complex32; AA.div_ceil(PIECE)]) {
        let mut pieces = [Complex32::new(0.0, 0.0); AA.div_ceil(PIECE)];
        let mut e = 0f32;
        let i0 = (n - self.base) as usize;
        for k in 0..AA {
            let v = self.y[i0 + SPS * k];
            pieces[k / PIECE] += v * self.aa[k];
            e += v.norm_sqr();
        }
        let p: f32 = pieces.iter().map(|c| c.norm_sqr()).sum();
        (p / (PIECE as f32 * e).max(1e-30), pieces)
    }

    /// Advances the search; true when a burst starts (or there's more to
    /// search), false when it needs more samples.
    fn search(&mut self) -> bool {
        let need = (SPS * (AA - 1)) as u64 + 1;
        while self.next + need <= self.end() {
            let n = self.next;
            self.next += 1;
            if n < self.busy_until || n < self.base + EQ_REACH as u64 {
                continue;
            }
            let (m, _) = self.metric(n);
            if self.trace && m > 0.12 {
                self.peaks.push((n, m));
            }
            if let Some((_, best)) = self.cand {
                if m > best {
                    self.cand = Some((n, m));
                }
            } else if m > Self::DETECT {
                self.cand = Some((n, m));
                self.cand_until = n + (SPS * 130) as u64;
            }
            if self.cand.is_some() && n >= self.cand_until {
                let (n0, m) = self.cand.take().unwrap();
                self.start(n0, m);
                return true;
            }
        }
        false
    }

    fn start(&mut self, n0: u64, detect: f32) {
        let (_, p) = self.metric(n0);
        // coarse: the pieces' phase steps; fine: a line through the
        // unwrapped phases
        let step: Complex32 = p.windows(2).map(|w| w[1] * w[0].conj()).sum();
        let coarse = step.arg() as f64 / (PIECE * SPS) as f64;
        let mut ph = Vec::with_capacity(p.len());
        let mut prev = 0f64;
        for (j, c) in p.iter().enumerate() {
            let expect = coarse * (j * PIECE * SPS) as f64;
            let mut v = c.arg() as f64 - expect;
            if j > 0 {
                v = prev + wrap(v - prev);
            }
            prev = v;
            ph.push(v + expect);
        }
        // weighted line fit (weights |piece|)
        let w: Vec<f64> = p.iter().map(|c| c.norm() as f64).collect();
        let xs: Vec<f64> = (0..p.len())
            .map(|j| ((j * PIECE + PIECE / 2) * SPS) as f64)
            .collect();
        let sw: f64 = w.iter().sum();
        let mx = xs.iter().zip(&w).map(|(x, w)| x * w).sum::<f64>() / sw;
        let my = ph.iter().zip(&w).map(|(y, w)| y * w).sum::<f64>() / sw;
        let sxy: f64 = xs
            .iter()
            .zip(&ph)
            .zip(&w)
            .map(|((x, y), w)| w * (x - mx) * (y - my))
            .sum();
        let sxx: f64 = xs.iter().zip(&w).map(|(x, w)| w * (x - mx).powi(2)).sum();
        let f = sxy / sxx / (2.0 * std::f64::consts::PI);
        self.rx = Some(Rx {
            n0,
            detect,
            f,
            w: [Complex32::new(0.0, 0.0); EQ],
            k: 0,
            theta: 0.0,
            nu: 0.0,
            mode: None,
            m1: 0.0,
            z_m1: Vec::new(),
            soft: Vec::new(),
            probes: Vec::new(),
        });
        self.busy_until = n0 + (SPS * (AA + phy::M1_LEN)) as u64;
    }

    /// The equaliser's input for symbol k (derotated).
    fn taps(&self, rx: &Rx, k: usize) -> [Complex32; EQ] {
        let c = rx.n0 + (SPS * k) as u64;
        std::array::from_fn(|i| {
            let n = c + (2 * i) as u64 - EQ_REACH as u64;
            let ph = -2.0 * std::f64::consts::PI * rx.f * (n as f64 - rx.n0 as f64);
            self.at(n) * Complex32::new(ph.cos() as f32, ph.sin() as f32)
        })
    }

    /// Least squares fit of the equaliser to the A pair.
    fn train(&self, rx: &mut Rx) {
        let mut r = [[Complex64::new(0.0, 0.0); EQ]; EQ];
        let mut b = [Complex64::new(0.0, 0.0); EQ];
        for k in 0..AA {
            let x = self
                .taps(rx, k)
                .map(|v| Complex64::new(v.re as f64, v.im as f64));
            let d = self.aa[k] as f64;
            for i in 0..EQ {
                // minimise Σ|d - Σ w_j x_j|²: R w = b, R_ij = Σ x_i* x_j
                for j in 0..EQ {
                    r[i][j] += x[i].conj() * x[j];
                }
                b[i] += x[i].conj() * d;
            }
        }
        let tr: f64 = (0..EQ).map(|i| r[i][i].re).sum::<f64>() / EQ as f64;
        for (i, row) in r.iter_mut().enumerate() {
            row[i] += tr * self.params.ridge;
        }
        let w = solve(r, b);
        rx.w = w.map(|v| Complex32::new(v.re as f32, v.im as f32));
    }

    /// Runs the burst as far as the samples go; None when it's done.
    fn run(&mut self, mut rx: Rx) -> Option<Rx> {
        let n0 = rx.n0;
        let avail = |k: usize| n0 + (SPS * k + EQ_REACH) as u64 + 1;
        if rx.k == 0 {
            if avail(AA) > self.end() {
                return Some(rx);
            }
            self.train(&mut rx);
            rx.k = AA;
        }
        loop {
            let total = rx.mode.map_or(AA + phy::M1_LEN, |m| m.symbols());
            if rx.k >= total {
                break;
            }
            if avail(rx.k) > self.end() {
                return Some(rx);
            }
            let x = self.taps(&rx, rx.k);
            let raw: Complex32 = x.iter().zip(&rx.w).map(|(a, b)| a * b).sum();
            let rot = Complex32::new(rx.theta.cos(), rx.theta.sin());
            let z = raw * rot.conj();
            let k = rx.k;
            // what the symbol is (or is decided to be), and the step size
            let mut known = true;
            let (d, mu) = match rx.mode {
                None => {
                    rx.z_m1.push(z);
                    known = false;
                    (Complex32::new(z.re.signum(), 0.0), self.params.mu_dd)
                }
                Some(m) => {
                    let s = k - AA - phy::M1_LEN;
                    if s < phy::M2_LEN {
                        (Complex32::new(bpsk(m.m1()[s]), 0.0), self.params.mu_known)
                    } else if s < phy::M2_LEN + phy::T_PREAMBLE * phy::T_LEN {
                        let d = Complex32::new(bpsk(phy::T[(s - phy::M2_LEN) % phy::T_LEN]), 0.0);
                        if s >= phy::M2_LEN + (phy::T_PREAMBLE - 1) * phy::T_LEN {
                            // the last: the first data block's probe before
                            rx.probes.push(z * d);
                        }
                        (d, self.params.mu_known)
                    } else {
                        let f = k - phy::PREAMBLE;
                        let i = f % (phy::DATA + phy::T_LEN);
                        if i >= phy::DATA {
                            let d = Complex32::new(bpsk(phy::T[i - phy::DATA]), 0.0);
                            rx.probes.push(z * d);
                            (d, self.params.mu_known)
                        } else {
                            rx.soft.push(z);
                            known = false;
                            (decide(z, m.bits), self.params.mu_dd)
                        }
                    }
                }
            };
            // phase loop
            let pe = if known || self.params.pll_dd {
                (z * d.conj()).arg()
            } else {
                0.0
            };
            rx.nu += self.params.pll_i * pe;
            rx.theta += self.params.pll * pe + rx.nu;
            // normalised LMS on the error (in the equaliser's frame)
            let e = d * rot - raw;
            let p: f32 = x.iter().map(|v| v.norm_sqr()).sum::<f32>() + 1e-12;
            for (w, v) in rx.w.iter_mut().zip(&x) {
                *w += v.conj() * e * (mu / p);
            }
            rx.k += 1;
            if rx.k == AA + phy::M1_LEN && rx.mode.is_none() {
                // which M1
                let e: f32 = rx.z_m1.iter().map(|z| z.norm()).sum();
                let (best, c) = (0..8)
                    .map(|i| {
                        let m1 = Mode::new(i).m1();
                        let c: f32 = rx.z_m1.iter().zip(m1).map(|(z, b)| z.re * bpsk(b)).sum();
                        (i, c / e.max(1e-30))
                    })
                    .max_by(|a, b| a.1.total_cmp(&b.1))
                    .unwrap();
                if c < Self::M1 {
                    self.false_starts += 1;
                    self.busy_until = rx.n0 + (SPS * AA) as u64;
                    return None;
                }
                let m = Mode::new(best);
                rx.mode = Some(m);
                rx.m1 = c;
                self.busy_until = rx.n0 + (SPS * m.symbols()) as u64;
            }
        }
        let m = rx.mode?;
        let (gains, sigma2) = channel(&rx.probes);
        let gains = if self.params.interp {
            gains
        } else {
            Vec::new()
        };
        let octets = decode(&rx.soft, &gains, m, sigma2);
        let p = if gains.is_empty() {
            1.0
        } else {
            gains.iter().map(|g| g.norm_sqr()).sum::<f32>() / gains.len() as f32
        };
        self.out.push(Burst {
            start: rx.n0,
            mode: m,
            freq: (rx.f * crate::front::FS) as f32,
            snr: 10.0 * (p / sigma2).log10(),
            detect: rx.detect,
            m1: rx.m1,
            octets,
            symbols: rx.soft,
            sigma2,
            gains,
        });
        None
    }
}

/// Each probe's gain (the mean of z·d over it) and the noise variance
/// about those gains.
pub fn channel(probes: &[Complex32]) -> (Vec<Complex32>, f32) {
    let gains: Vec<Complex32> = probes
        .chunks(phy::T_LEN)
        .map(|c| c.iter().sum::<Complex32>() / c.len() as f32)
        .collect();
    let mut e = 0f64;
    for (c, g) in probes.chunks(phy::T_LEN).zip(&gains) {
        e += c.iter().map(|v| (v - g).norm_sqr() as f64).sum::<f64>();
    }
    // (each block's mean takes one of its 15 degrees of freedom)
    let n = probes.len() as f64 * (phy::T_LEN - 1) as f64 / phy::T_LEN as f64;
    (gains, (e / n.max(1.0)).max(1e-6) as f32)
}

/// The gain at data symbol n: between the probe before its block and
/// the one after (their centres 8 and 37 symbols from the block's start).
fn gain_at(gains: &[Complex32], n: usize) -> Complex32 {
    if gains.len() < 2 {
        return Complex32::new(1.0, 0.0);
    }
    let (j, i) = (n / phy::DATA, n % phy::DATA);
    let (a, b) = (
        gains[j.min(gains.len() - 1)],
        gains[(j + 1).min(gains.len() - 1)],
    );
    let w = (i as f32 + 8.0) / (phy::DATA + phy::T_LEN) as f32;
    a * (1.0 - w) + b * w
}

/// Soft bits (positive for 0) in the received order, descrambled; the
/// constellation scaled by the gain between the probes (`gains`, none: 1).
pub fn soft_bits(soft: &[Complex32], gains: &[Complex32], m: Mode, sigma2: f32) -> Vec<f32> {
    let scr = phy::scrambler();
    let npt = 1usize << m.bits;
    let pts: Vec<Complex32> = (0..npt)
        .map(|v| {
            let a = 2.0 * std::f32::consts::PI * gray_decode(v) as f32 / npt as f32;
            Complex32::new(a.cos(), a.sin())
        })
        .collect();
    let mut llr = Vec::with_capacity(m.coded());
    for (i, &z) in soft.iter().enumerate() {
        let z = if scr[i % 120] == 1 { -z } else { z };
        let g = gain_at(gains, i);
        let d: Vec<f32> = pts.iter().map(|p| (z - g * p).norm_sqr()).collect();
        for b in (0..m.bits).rev() {
            let (mut d0, mut d1) = (f32::INFINITY, f32::INFINITY);
            for (v, &dv) in d.iter().enumerate() {
                if v >> b & 1 == 0 {
                    d0 = d0.min(dv);
                } else {
                    d1 = d1.min(dv);
                }
            }
            llr.push((d1 - d0) / sigma2);
        }
    }
    llr
}

/// Data symbols to octets.
pub fn decode(soft: &[Complex32], gains: &[Complex32], m: Mode, sigma2: f32) -> Vec<u8> {
    let llr = soft_bits(soft, gains, m, sigma2);
    let perm = phy::interleave(&m);
    let de: Vec<f32> = perm.iter().map(|&p| llr[p as usize]).collect();
    let de = if m.repeat {
        de.chunks(2).map(|c| c[0] + c[1]).collect()
    } else {
        de
    };
    phy::octets(&phy::viterbi(&de))
}

fn wrap(x: f64) -> f64 {
    let t = std::f64::consts::TAU;
    x - t * (x / t).round()
}

fn gray_decode(g: usize) -> usize {
    let mut b = g;
    let mut s = g >> 1;
    while s != 0 {
        b ^= s;
        s >>= 1;
    }
    b
}

/// Nearest point of M-PSK (2^bits points, one at 0°).
fn decide(z: Complex32, bits: usize) -> Complex32 {
    let m = (1usize << bits) as f32;
    let step = std::f32::consts::TAU / m;
    let a = (z.arg() / step).round() * step;
    Complex32::new(a.cos(), a.sin())
}

/// Solves R w = b (R Hermitian positive definite) by Gaussian elimination.
#[allow(clippy::needless_range_loop)]
fn solve<const N: usize>(mut r: [[Complex64; N]; N], mut b: [Complex64; N]) -> [Complex64; N] {
    for c in 0..N {
        let p = (c..N)
            .max_by(|&i, &j| r[i][c].norm().total_cmp(&r[j][c].norm()))
            .unwrap();
        r.swap(c, p);
        b.swap(c, p);
        let d = r[c][c];
        for i in c + 1..N {
            let f = r[i][c] / d;
            for j in c..N {
                let v = r[c][j];
                r[i][j] -= f * v;
            }
            let v = b[c];
            b[i] -= f * v;
        }
    }
    let mut w = [Complex64::new(0.0, 0.0); N];
    for i in (0..N).rev() {
        let s: Complex64 = (i + 1..N).map(|j| r[i][j] * w[j]).sum();
        w[i] = (b[i] - s) / r[i][i];
    }
    w
}
