// SPDX-License-Identifier: GPL-3.0-or-later
//! FT4 decoder for one 7.5-second slot of 12 kS/s audio.
//!
//! The same steps as the FT8 decoder (`ft8.rs`), for FT4's signal: 4-GFSK
//! at 20.8 Bd (BT 1.0), 103 symbols with four 4×4 Costas arrays at
//! symbols 0, 33, 66 and 99, and a ramp symbol before and after. The 77
//! message bits are scrambled with a fixed sequence before the CRC.
//!
//! 1. **Candidates** on a spectrogram (symbol-length windows, 1/4-symbol
//!    steps, 1/2-tone bins) from the four Costas arrays.
//! 2. **Per candidate**, the slot is shifted down and taken at 666.7 S/s
//!    (32 samples per symbol); time and frequency are refined on the
//!    Costas tones, then each symbol's 4 tone amplitudes taken.
//! 3. **Soft bits** from 1, 2 and 4 symbols coherently, belief
//!    propagation, then OSD.
//! 4. **Subtraction** of what was decoded, and further passes.

use crate::ft8::{boxcar, delay, osd_plausible, parabola_peak};
use crate::ldpc::{self, Bp, N};
use crate::msg::{self, CallHashes, Message};
use crate::osd;
use crate::realfft::RealFft;
use crate::tones::{self, FT4_COSTAS, FT4_GRAY, FT4_SYMBOLS, Shape};
use num_complex::Complex32 as C32;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;
use web888_clock::Instant;

pub const RATE: f32 = 12_000.0;
pub const NSPS: usize = 576;
pub const BAUD: f32 = RATE / NSPS as f32;
/// Samples in a slot.
pub const SLOT: usize = 90_000;
/// Nominal start of a transmission (its ramp symbol) in the slot.
pub const T0: f32 = 0.5;

/// Symbols of the waveform: the channel symbols and a ramp at each end.
const NSYM: usize = FT4_SYMBOLS + 2;
/// Costas arrays at these channel symbols.
const SYNC_AT: [usize; 4] = [0, 33, 66, 99];
/// Data symbols start here, 29 each.
const DATA_AT: [usize; 3] = [4, 37, 70];

const NFFT1: usize = SLOT;
const DF1: f32 = RATE / NFFT1 as f32;
const DS_NSPS: usize = 32;
const DS_RATE: f32 = BAUD * DS_NSPS as f32;
const NFFT2: usize = NFFT1 * DS_NSPS / NSPS;

/// Frequency offsets tried in the fine sync, Hz (the coarse bins are
/// half a tone, 10.4 Hz).
const FINE_DF: [f32; 13] = [
    -6.0, -5.0, -4.0, -3.0, -2.0, -1.0, 0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0,
];
const FINE_STEP: f32 = 1.0;

static TAPER: std::sync::LazyLock<[f32; 101]> = std::sync::LazyLock::new(|| {
    std::array::from_fn(|k| 0.5 * (1.0 - (std::f32::consts::PI * k as f32 / 101.0).cos()))
});

/// Band subtracted around tone 0 of a decoded signal, Hz.
const SUB_BELOW: f32 = 60.0;
const SUB_ABOVE: f32 = 125.0;

/// The spectrogram works on the band 0..5333 Hz at 5333 S/s: 256
/// samples per symbol, 1/4-symbol steps, 1/2-tone bins.
const SPEC_NSPS: usize = 256;
const SPEC_N: usize = NFFT1 * SPEC_NSPS / NSPS;
const SPEC_STEP: usize = SPEC_NSPS / 4;
const SPEC_NFFT: usize = 2 * SPEC_NSPS;
const SPEC_DF: f32 = BAUD / 2.0;
const SPEC_EDGE: f32 = 100.0;
/// Seconds per spectrogram step.
const SPEC_DT: f32 = NSPS as f32 / 4.0 / RATE;
/// Coarse start times searched: this far either side of the nominal.
const MAX_LAG_S: f32 = 1.0;

#[derive(Clone, Debug)]
pub struct Config {
    pub f_min: f32,
    pub f_max: f32,
    pub sync_min: f32,
    pub max_candidates: usize,
    /// Costas symbols (of 16) that must have their tone on top at the
    /// coarse time peak, before the fine sync is spent on the candidate,
    /// and after it, before the soft bits and the decoders. No decode of
    /// the corpora has fewer (doc/m4-skimmers.md §3.3).
    pub sync_symbols_coarse: usize,
    pub sync_symbols: usize,
    /// The same count taken from the search's spectrogram (of the 16, or
    /// in proportion for a signal partly outside the slot), before the
    /// candidate is downsampled at all.
    pub sync_symbols_search: usize,
    pub passes: usize,
    pub bp_iterations: usize,
    pub osd: Option<osd::Depth>,
    pub osd_max_hard_errors: usize,
    pub osd_max_distance: f32,
    pub osd_max_ratio: f32,
    pub osd_max_ratio_known: f32,
    pub llr_scale: f32,
    /// Log what happens to candidates within 10 Hz of this frequency.
    pub trace: Option<f32>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            f_min: 200.0,
            f_max: 3000.0,
            sync_min: 1.5,
            max_candidates: 300,
            sync_symbols_coarse: 7,
            sync_symbols: 8,
            sync_symbols_search: 6,
            passes: 3,
            bp_iterations: 30,
            osd: Some(osd::Depth {
                order1: 91,
                order2: 91,
                order3: 91,
            }),
            osd_max_hard_errors: 52,
            osd_max_distance: 90.0,
            osd_max_ratio: 0.65,
            osd_max_ratio_known: 0.75,
            llr_scale: 5.5,
            trace: None,
        }
    }
}

pub use crate::ft8::{Decode, Stats};

#[derive(Clone, Copy, Debug)]
struct Candidate {
    freq: f32,
    dt: f32,
    sync: f32,
}

struct Demod {
    f0: f32,
    t: f32,
    sync: f32,
    cs: Vec<[C32; 4]>,
    s4: Vec<[f32; 4]>,
    nsync: usize,
}

pub struct Decoder {
    cfg: Config,
    fft1: RealFft,
    fft2: Arc<dyn Fft<f32>>,
    ifft2: Arc<dyn Fft<f32>>,
    spec_fft: Arc<dyn Fft<f32>>,
    spec_ifft: Arc<dyn Fft<f32>>,
    fft32: Arc<dyn Fft<f32>>,
    bp: Bp,
    tone_ref: [[C32; DS_NSPS]; 4],
    fine_tw: [[C32; DS_NSPS]; FINE_DF.len()],
    pub stats: Stats,
}

impl Decoder {
    pub fn new(cfg: Config) -> Self {
        let mut p = FftPlanner::new();
        let tone_ref = std::array::from_fn(|t| {
            std::array::from_fn(|n| {
                let ph = 2.0 * std::f32::consts::PI * t as f32 * n as f32 / DS_NSPS as f32;
                C32::new(ph.cos(), ph.sin())
            })
        });
        Decoder {
            cfg,
            fft1: RealFft::new(NFFT1),
            fft2: p.plan_fft_forward(NFFT2),
            ifft2: p.plan_fft_inverse(NFFT2),
            spec_fft: p.plan_fft_forward(SPEC_NFFT),
            spec_ifft: p.plan_fft_inverse(SPEC_N),
            fft32: p.plan_fft_forward(DS_NSPS),
            bp: Bp::default(),
            tone_ref,
            fine_tw: std::array::from_fn(|k| {
                std::array::from_fn(|n| {
                    let ph = -2.0 * std::f32::consts::PI * FINE_DF[k] * n as f32 / DS_RATE;
                    C32::new(ph.cos(), ph.sin())
                })
            }),
            stats: Stats::default(),
        }
    }

    pub fn decode(&mut self, audio: &[f32], hashes: &mut CallHashes) -> Vec<Decode> {
        self.decode_until(audio, hashes, None)
    }

    /// Decode a slot (`audio` from the slot boundary, up to 7.5 s), as
    /// `ft8::Decoder::decode_until`.
    pub fn decode_until(
        &mut self,
        audio: &[f32],
        hashes: &mut CallHashes,
        deadline: Option<Instant>,
    ) -> Vec<Decode> {
        let mut x = vec![0f32; SLOT];
        let n = audio.len().min(SLOT);
        x[..n].copy_from_slice(&audio[..n]);
        let mut found: Vec<Decode> = vec![];
        self.stats.slots += 1;
        let mut spectrum = self.fft1.forward(&x);
        drop(x);
        let mut tried: Vec<(f32, f32)> = vec![];
        'passes: for pass in 1..=self.cfg.passes {
            let t = Instant::now();
            let cands = self.candidates(&spectrum);
            self.stats.search += t.elapsed();
            self.stats.candidates += cands.len() as u64;
            let mut new: Vec<Decode> = vec![];
            for c in cands {
                if deadline.is_some_and(|d| Instant::now() >= d) {
                    self.stats.cut_short += 1;
                    found.extend(new);
                    break 'passes;
                }
                let near_tried = tried
                    .iter()
                    .any(|&(f, t)| (f - c.freq).abs() < 5.0 && (t - c.dt).abs() < 0.02);
                let near_new = found.iter().any(|d| {
                    d.pass as usize == pass - 1
                        && c.freq > d.freq - SUB_BELOW
                        && c.freq < d.freq + SUB_ABOVE
                });
                if near_tried && !near_new {
                    continue;
                }
                tried.push((c.freq, c.dt));
                if new
                    .iter()
                    .any(|d| (d.freq - c.freq).abs() < 12.0 && (d.dt - c.dt).abs() < 0.05)
                {
                    continue;
                }
                if let Some(mut d) = self.decode_candidate(&spectrum, c, hashes) {
                    if found.iter().chain(new.iter()).any(|e| e.bits == d.bits) {
                        continue;
                    }
                    d.pass = pass as u8;
                    let t = Instant::now();
                    self.subtract(&mut spectrum, &d);
                    self.stats.subtract += t.elapsed();
                    new.push(d);
                }
            }
            if new.is_empty() {
                break;
            }
            found.extend(new);
        }
        found
    }

    /// Real audio back from a slot spectrum (tests).
    #[cfg(test)]
    fn audio(&self, spectrum: &[C32]) -> Vec<f32> {
        let mut x = self.fft1.inverse(spectrum);
        x.truncate(SLOT);
        x.iter_mut().for_each(|v| *v /= NFFT1 as f32);
        x
    }

    /// Remove a decoded signal from the slot spectrum (as FT8's).
    fn subtract(&self, spectrum: &mut [C32], d: &Decode) {
        let t = (d.dt + T0) * DS_RATE;
        let i0 = t.floor();
        let frac = (t - i0) / DS_RATE;
        let i0 = i0 as i64;
        let cd = self.downsample_shifted(spectrum, d.freq, frac);
        let reference = tones::synth_complex(
            &tones::ft4(d.bits),
            BAUD as f64,
            Shape::Ft4,
            0.0,
            DS_RATE as f64,
        );
        let at = |n: usize| (i0 + n as i64).rem_euclid(NFFT2 as i64) as usize;
        let camp: Vec<C32> = reference
            .iter()
            .enumerate()
            .map(|(n, r)| cd[at(n)] * r.conj())
            .collect();
        // Two 0.1 s moving averages, as a least-squares fit: divided by
        // the reference's own power, which the ramp symbols take to zero.
        let w = (0.1 * DS_RATE) as usize | 1;
        let power: Vec<C32> = reference
            .iter()
            .map(|r| C32::new(r.norm_sqr(), 0.0))
            .collect();
        let num = boxcar(&boxcar(&camp, w), w);
        let den = boxcar(&boxcar(&power, w), w);
        let amp: Vec<C32> = num
            .iter()
            .zip(&den)
            .map(|(n, d)| n / d.re.max(1e-6))
            .collect();
        let mut s = vec![C32::default(); NFFT2];
        for (n, (a, r)) in amp.iter().zip(&reference).enumerate() {
            s[at(n)] = a * r;
        }
        self.fft2.process(&mut s);
        let scale = ((NFFT1 * NFFT2) as f32).sqrt() / NFFT2 as f32;
        let ib0 = (d.freq / DF1).round() as i64;
        let ib = (ib0 - (SUB_BELOW / DF1) as i64).max(1);
        let it = (ib0 + (SUB_ABOVE / DF1) as i64).min(spectrum.len() as i64 - 1);
        for bin in ib..=it {
            let k = bin - ib0;
            let ph = -2.0 * std::f32::consts::PI * k as f32 * DF1 * frac;
            let v = s[k.rem_euclid(NFFT2 as i64) as usize] * C32::new(ph.cos(), ph.sin()) * scale;
            spectrum[bin as usize] -= v;
        }
    }

    /// The slot's lower band as a complex signal for the spectrogram (as
    /// FT8's).
    fn baseband(&self, spectrum: &[C32]) -> Vec<C32> {
        let mut c: Vec<C32> = spectrum[..SPEC_N].to_vec();
        // Raised-cosine edges, SPEC_EDGE Hz wide.
        let ne = (SPEC_EDGE / DF1) as usize;
        for k in 0..ne {
            let w = 0.5 * (1.0 - (std::f32::consts::PI * k as f32 / ne as f32).cos());
            c[k] *= w;
            c[SPEC_N - 1 - k] *= w;
        }
        self.spec_ifft.process(&mut c);
        c
    }

    /// Coarse search on a spectrogram (as FT8's, with FT4's sync).
    fn candidates(&self, spectrum: &[C32]) -> Vec<Candidate> {
        let nsteps = SLOT / (NSPS / 4) - 3;
        let i_min = (self.cfg.f_min / SPEC_DF) as usize;
        let top = ((RATE * SPEC_NSPS as f32 / NSPS as f32 - SPEC_EDGE) / SPEC_DF) as usize;
        let i_max = ((self.cfg.f_max / SPEC_DF) as usize).min(top - 8);
        let nbins = i_max + 8;
        let z = self.baseband(spectrum);
        let mut s = vec![0f32; nsteps * nbins];
        let mut buf = vec![C32::default(); SPEC_NFFT];
        for j in 0..nsteps {
            buf[..SPEC_NSPS].copy_from_slice(&z[j * SPEC_STEP..][..SPEC_NSPS]);
            buf[SPEC_NSPS..].fill(C32::default());
            self.spec_fft.process(&mut buf);
            for (v, b) in s[j * nbins..][..nbins].iter_mut().zip(&buf) {
                *v = b.norm_sqr();
            }
        }
        let mut tsum = vec![0f32; nsteps * nbins];
        for j in 0..nsteps {
            let row = &s[j * nbins..(j + 1) * nbins];
            for i in 0..nbins - 6 {
                tsum[j * nbins + i] = (0..4).map(|t| row[i + 2 * t]).sum();
            }
        }
        let step_s = SPEC_DT;
        let j0 = (T0 / step_s).round() as i32;
        let max_lag = (MAX_LAG_S / step_s).round() as i32;
        let ni = i_max - i_min;
        let mut best = vec![(0f32, 0i32, 0f32, 0i32); ni];
        let mut ta = vec![[0f32; 4]; ni];
        let mut t0 = vec![[0f32; 4]; ni];
        for lag in -max_lag..=max_lag {
            ta.iter_mut().for_each(|v| *v = [0.0; 4]);
            t0.iter_mut().for_each(|v| *v = [0.0; 4]);
            for (a, &at) in SYNC_AT.iter().enumerate() {
                for (k, &tone) in FT4_COSTAS[a].iter().enumerate() {
                    // +1: the ramp symbol
                    let j = j0 + lag + 4 * (1 + at + k) as i32;
                    if j < 0 || j as usize >= nsteps {
                        continue;
                    }
                    let base = j as usize * nbins + i_min;
                    let row = &s[base + 2 * tone as usize..][..ni];
                    let trow = &tsum[base..][..ni];
                    for ((ta, t0), (&v, &tv)) in
                        ta.iter_mut().zip(t0.iter_mut()).zip(row.iter().zip(trow))
                    {
                        ta[a] += v;
                        t0[a] += tv;
                    }
                }
            }
            for ((b, ta), t0) in best.iter_mut().zip(&ta).zip(&t0) {
                let score = |ta: f32, t0: f32| if t0 > ta { ta / ((t0 - ta) / 3.0) } else { 0.0 };
                let all = score(ta.iter().sum(), t0.iter().sum());
                let first = score(ta[0] + ta[1] + ta[2], t0[0] + t0[1] + t0[2]);
                let last = score(ta[1] + ta[2] + ta[3], t0[1] + t0[2] + t0[3]);
                let sync = all.max(first).max(last);
                if sync > b.0 {
                    b.0 = sync;
                    b.1 = lag;
                }
                if lag.abs() <= 10 && sync > b.2 {
                    b.2 = sync;
                    b.3 = lag;
                }
            }
        }
        let mut sorted: Vec<f32> = best.iter().map(|b| b.0).collect();
        sorted.sort_by(f32::total_cmp);
        let base = sorted
            .get(sorted.len() * 2 / 5)
            .copied()
            .unwrap_or(1.0)
            .max(1e-9);
        let mut cands = vec![];
        for (k, &(s1, l1, s2, l2)) in best.iter().enumerate() {
            let freq = (i_min + k) as f32 * SPEC_DF;
            for (sv, lag) in [(s1, l1), (s2, l2)] {
                let sync = sv / base;
                if sync >= self.cfg.sync_min {
                    cands.push(Candidate {
                        freq,
                        dt: lag as f32 * step_s,
                        sync,
                    });
                }
            }
        }
        cands.sort_by(|a, b| b.sync.total_cmp(&a.sync));
        let mut kept: Vec<Candidate> = vec![];
        for c in cands {
            if kept
                .iter()
                .any(|k| (k.freq - c.freq).abs() < 12.0 && (k.dt - c.dt).abs() < step_s + 1e-3)
            {
                continue;
            }
            // The hard sync check on the spectrogram (`sync_symbols_search`).
            let i = (c.freq / SPEC_DF).round() as usize;
            let lag = (c.dt / step_s).round() as i32;
            let (mut hits, mut seen) = (0, 0);
            for (a, &at) in SYNC_AT.iter().enumerate() {
                for (k, &tone) in FT4_COSTAS[a].iter().enumerate() {
                    let j = j0 + lag + 4 * (1 + at + k) as i32;
                    if j < 0 || j as usize >= nsteps {
                        continue;
                    }
                    let row = &s[j as usize * nbins + i..];
                    let top = (0..4)
                        .max_by(|&x, &y| row[2 * x].total_cmp(&row[2 * y]))
                        .unwrap();
                    seen += 1;
                    hits += (top == tone as usize) as usize;
                }
            }
            if hits * 16 < self.cfg.sync_symbols_search * seen {
                continue;
            }
            kept.push(c);
            if kept.len() >= self.cfg.max_candidates {
                break;
            }
        }
        kept
    }

    fn downsample(&self, spectrum: &[C32], f0: f32) -> Vec<C32> {
        self.downsample_shifted(spectrum, f0, 0.0)
    }

    /// The slot around `f0` at 666.7 S/s, tone 0 at 0 Hz, advanced by
    /// `shift` seconds.
    fn downsample_shifted(&self, spectrum: &[C32], f0: f32, shift: f32) -> Vec<C32> {
        let i0 = (f0 / DF1).round() as i64;
        let ib = (i0 - (1.5 * BAUD / DF1) as i64).max(1);
        let it = (i0 + (4.5 * BAUD / DF1) as i64).min(spectrum.len() as i64 - 1);
        let mut c = vec![C32::default(); NFFT2];
        let nb = (it - ib + 1) as usize;
        let taper = TAPER.len().min(nb / 2);
        let ramp = 2.0 * std::f32::consts::PI * DF1 * shift;
        let step = C32::new(ramp.cos(), ramp.sin());
        let ph0 = ramp * (ib - i0) as f32;
        let mut rot = C32::new(ph0.cos(), ph0.sin());
        for k in 0..nb {
            let mut v = spectrum[(ib + k as i64) as usize];
            if k < taper {
                v *= TAPER[k * TAPER.len() / taper];
            } else if k >= nb - taper {
                v *= TAPER[(nb - 1 - k) * TAPER.len() / taper];
            }
            if shift != 0.0 {
                v *= rot;
                rot *= step;
            }
            let dst = ((ib + k as i64 - i0).rem_euclid(NFFT2 as i64)) as usize;
            c[dst] = v;
        }
        self.ifft2.process(&mut c);
        let scale = 1.0 / ((NFFT1 * NFFT2) as f32).sqrt();
        for v in c.iter_mut() {
            *v *= scale;
        }
        c
    }

    /// Start (in samples at 666.7 S/s) of channel symbol `k` of a signal
    /// whose ramp symbol starts at `i0`.
    fn sym_start(i0: i64, k: usize) -> i64 {
        i0 + (DS_NSPS * (1 + k)) as i64
    }

    /// Coherent Costas correlation at `n` consecutive start samples from
    /// `first`, by a sliding DFT per sync symbol (as FT8's `sync_lags`).
    fn sync_lags(&self, cd: &[C32], first: i64, n: usize) -> Vec<f32> {
        let at = |i: i64| {
            if i >= 0 && (i as usize) < cd.len() {
                cd[i as usize]
            } else {
                C32::default()
            }
        };
        let mut sum = vec![0f32; n];
        for (a, &sym) in SYNC_AT.iter().enumerate() {
            for (k, &tone) in FT4_COSTAS[a].iter().enumerate() {
                let r = &self.tone_ref[tone as usize];
                let step = r[1];
                let start = Self::sym_start(first, sym + k);
                let mut x: C32 = (0..DS_NSPS)
                    .map(|j| at(start + j as i64) * r[j].conj())
                    .sum();
                for (lag, s) in sum.iter_mut().enumerate() {
                    if lag > 0 {
                        let s0 = start + lag as i64 - 1;
                        x = (x - at(s0) + at(s0 + DS_NSPS as i64)) * step;
                    }
                    *s += x.norm_sqr();
                }
            }
        }
        sum
    }

    /// Coherent Costas correlation at start sample `i0` for each of the
    /// frequency offsets in `FINE_DF`.
    fn sync_fine_df(
        &self,
        cd: &[C32],
        i0: i64,
        offsets: &[[C32; DS_NSPS]; FINE_DF.len()],
    ) -> [f32; FINE_DF.len()] {
        let mut sum = [0f32; FINE_DF.len()];
        let mut p = [C32::default(); DS_NSPS];
        for (a, &at) in SYNC_AT.iter().enumerate() {
            for (k, &tone) in FT4_COSTAS[a].iter().enumerate() {
                let start = Self::sym_start(i0, at + k);
                for (n, (pn, r)) in p.iter_mut().zip(&self.tone_ref[tone as usize]).enumerate() {
                    let idx = start + n as i64;
                    *pn = if idx >= 0 && (idx as usize) < cd.len() {
                        cd[idx as usize] * r.conj()
                    } else {
                        C32::default()
                    };
                }
                for (s, t) in sum.iter_mut().zip(offsets) {
                    *s += p.iter().zip(t).map(|(a, b)| a * b).sum::<C32>().norm_sqr();
                }
            }
        }
        sum
    }

    /// How many of the 16 Costas symbols of a signal whose ramp symbol
    /// starts at `i0` have their tone on top.
    fn sync_hits(&self, cd: &[C32], i0: i64) -> usize {
        let mut n = 0;
        let mut buf = [C32::default(); DS_NSPS];
        for (a, &at) in SYNC_AT.iter().enumerate() {
            for (k, &tone) in FT4_COSTAS[a].iter().enumerate() {
                let start = Self::sym_start(i0, at + k);
                for (j, b) in buf.iter_mut().enumerate() {
                    let idx = start + j as i64;
                    *b = if idx >= 0 && (idx as usize) < cd.len() {
                        cd[idx as usize]
                    } else {
                        C32::default()
                    };
                }
                self.fft32.process(&mut buf);
                let top = (0..4)
                    .max_by(|&x, &y| buf[x].norm_sqr().total_cmp(&buf[y].norm_sqr()))
                    .unwrap();
                n += (top == tone as usize) as usize;
            }
        }
        n
    }

    /// Each channel symbol's 4 tone amplitudes, and how many Costas
    /// symbols have their tone on top.
    fn symbols(&self, cd: &[C32], i0: i64) -> (Vec<[C32; 4]>, Vec<[f32; 4]>, usize) {
        let mut cs = vec![[C32::default(); 4]; FT4_SYMBOLS];
        let mut buf = [C32::default(); DS_NSPS];
        for (k, row) in cs.iter_mut().enumerate() {
            let start = Self::sym_start(i0, k);
            for (n, b) in buf.iter_mut().enumerate() {
                let idx = start + n as i64;
                *b = if idx >= 0 && (idx as usize) < cd.len() {
                    cd[idx as usize]
                } else {
                    C32::default()
                };
            }
            self.fft32.process(&mut buf);
            row.copy_from_slice(&buf[..4]);
        }
        let s4: Vec<[f32; 4]> = cs
            .iter()
            .map(|r| std::array::from_fn(|t| r[t].norm()))
            .collect();
        let mut nsync = 0;
        for (a, &at) in SYNC_AT.iter().enumerate() {
            for k in 0..4 {
                let row = &s4[at + k];
                let top = (0..4).max_by(|&x, &y| row[x].total_cmp(&row[y])).unwrap();
                if top == FT4_COSTAS[a][k] as usize {
                    nsync += 1;
                }
            }
        }
        (cs, s4, nsync)
    }

    /// Refine a candidate and take its symbols' tone amplitudes. `None`
    /// for one with fewer than `coarse_min` Costas symbols at the coarse
    /// time peak (as FT8's).
    fn demod(&mut self, spectrum: &[C32], c: Candidate, coarse_min: usize) -> Option<Demod> {
        let t = Instant::now();
        let mut f0 = c.freq;
        let mut cd = self.downsample(spectrum, f0);
        let nominal = ((c.dt + T0) * DS_RATE).round() as i64;
        // The peak of sync over start samples first..first + len (the
        // first of equal ones).
        let peak = |v: &[f32], first: i64| {
            let mut best = (first, 0f32);
            for (k, &s) in v.iter().enumerate() {
                if s > best.1 {
                    best = (first + k as i64, s);
                }
            }
            best
        };
        // The coarse step is 1/4 symbol (8 samples): ±12 samples, and a
        // neighbour on each side for the sub-sample peak.
        let mut lags = (nominal - 13, self.sync_lags(&cd, nominal - 13, 27));
        let (mut i0, mut best) = peak(&lags.1[1..26], nominal - 12);
        if coarse_min > 0 && self.sync_hits(&cd, i0) < coarse_min {
            self.stats.sync += t.elapsed();
            if self.cfg.trace.is_some_and(|f| (f - c.freq).abs() < 10.0) {
                eprintln!("cand {c:?}\n  too few sync symbols at the coarse peak");
            }
            return None;
        }
        let fs = self.sync_fine_df(&cd, i0, &self.fine_tw);
        let kb = (0..fs.len())
            .max_by(|&a, &b| fs[a].total_cmp(&fs[b]))
            .unwrap();
        let mut df = FINE_DF[kb];
        if kb > 0 && kb < fs.len() - 1 {
            df += FINE_STEP * parabola_peak(fs[kb - 1], fs[kb], fs[kb + 1]);
        }
        if df.abs() > 0.1 {
            f0 += df;
            // Downsampled again: FT8's rotation of the samples costs FT4
            // decodes (3 of 736 on the corpora). The sub-sample step
            // below is a short filter, as FT8's.
            cd = self.downsample(spectrum, f0);
            let start = i0;
            lags = (start - 5, self.sync_lags(&cd, start - 5, 11));
            (i0, best) = peak(&lags.1[1..10], start - 4);
        }
        let near = |i: i64| lags.1[(i - lags.0) as usize];
        let p = parabola_peak(near(i0 - 1), best, near(i0 + 1));
        let frac = p.clamp(-0.5, 0.5);
        if frac.abs() > 0.05 {
            cd = delay(&cd, frac);
        }
        self.stats.sync += t.elapsed();
        let t = Instant::now();
        let (cs, s4, nsync) = self.symbols(&cd, i0);
        if self.cfg.trace.is_some_and(|f| (f - c.freq).abs() < 10.0) {
            let hard: String = s4
                .iter()
                .map(|r| {
                    (b'0' + (0..4).max_by(|&x, &y| r[x].total_cmp(&r[y])).unwrap() as u8) as char
                })
                .collect();
            eprintln!(
                "cand {c:?}\n  fine: f0 {f0:.2} i0 {i0} sync {best:.3e} nsync {nsync}\n  hard {hard}"
            );
        }
        self.stats.metrics += t.elapsed();
        Some(Demod {
            f0,
            t: (i0 as f32 + frac) / DS_RATE,
            sync: best,
            cs,
            s4,
            nsync,
        })
    }

    fn decode_candidate(
        &mut self,
        spectrum: &[C32],
        c: Candidate,
        hashes: &mut CallHashes,
    ) -> Option<Decode> {
        let Demod {
            f0,
            t: t_start,
            sync,
            cs,
            s4,
            nsync,
        } = self.demod(spectrum, c, self.cfg.sync_symbols_coarse)?;
        if nsync < self.cfg.sync_symbols {
            return None;
        }
        let t = Instant::now();
        let metrics: [[f32; N]; 3] = [1, 2, 4].map(|nsym| soft_bits(&cs, nsym, self.cfg.llr_scale));
        self.stats.metrics += t.elapsed();
        let t = Instant::now();
        for (m, how) in metrics.iter().zip([1u8, 2, 4]) {
            let d = self.bp.decode(m, self.cfg.bp_iterations);
            if d.errors == 0
                && let Some(r) = self.accept(&d.codeword, None, &s4, f0, t_start, sync, how, hashes)
            {
                self.stats.bp += t.elapsed();
                return Some(r);
            }
        }
        self.stats.bp += t.elapsed();
        let t = Instant::now();
        let mut result = None;
        if let Some(depth) = self.cfg.osd {
            for (m, how) in metrics.iter().zip([1u8, 2, 4]) {
                if let Some(f) = osd::decode(m, depth) {
                    result = self
                        .accept(
                            &f.codeword,
                            Some(&f),
                            &s4,
                            f0,
                            t_start,
                            sync,
                            how + 10,
                            hashes,
                        )
                        .map(|mut d| {
                            d.osd_distance = f.distance;
                            d.hard_errors = f.hard_errors;
                            d.osd_second = f.second;
                            d
                        });
                    if result.is_some() {
                        break;
                    }
                }
            }
        }
        self.stats.osd += t.elapsed();
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn accept(
        &self,
        cw: &ldpc::Bits,
        osd: Option<&osd::Found>,
        s4: &[[f32; 4]],
        f0: f32,
        t_start: f32,
        sync: f32,
        how: u8,
        hashes: &mut CallHashes,
    ) -> Option<Decode> {
        let scrambled = crate::crc::check(ldpc::message(cw))?;
        let bits = tones::ft4_scramble(scrambled);
        if bits == 0 {
            return None;
        }
        let message: Message = msg::peek(bits, hashes)?;
        let c = &self.cfg;
        if let Some(f) = osd
            && !osd_plausible(
                &message,
                f,
                hashes,
                c.osd_max_hard_errors,
                c.osd_max_distance,
                c.osd_max_ratio,
                c.osd_max_ratio_known,
            )
        {
            return None;
        }
        for c in &message.calls {
            hashes.save(c);
        }
        let mut xsig = 0.0;
        let mut xnoi = 0.0;
        for (k, &t) in tones::ft4(bits).iter().enumerate() {
            xsig += s4[k][t as usize].powi(2);
            xnoi += s4[k][(t as usize + 2) % 4].powi(2);
        }
        let ratio = (xsig / xnoi.max(1e-12) - 1.0).max(1e-3);
        // 2500 Hz against the 20.8 Hz of a tone
        let snr = (10.0 * ratio.log10() - 20.0).max(-24.0);
        Some(Decode {
            message,
            bits,
            freq: f0,
            dt: t_start - T0,
            snr,
            sync,
            how,
            pass: 0,
            osd_distance: 0.0,
            hard_errors: 0,
            osd_second: 0.0,
        })
    }
}

/// Bit log-likelihoods from coherent sums over groups of `nsym` symbols.
fn soft_bits(cs: &[[C32; 4]], nsym: usize, scale: f32) -> [f32; N] {
    let mut out = [0f32; N];
    let mut s2 = vec![0f32; 1 << (2 * nsym)];
    for (g, &base) in DATA_AT.iter().enumerate() {
        let mut ks = 0;
        while ks < 29 {
            let ns = nsym.min(29 - ks);
            let nt = 1usize << (2 * ns);
            for (i, v) in s2[..nt].iter_mut().enumerate() {
                let mut z = C32::default();
                for j in 0..ns {
                    let tone_bits = (i >> (2 * (ns - 1 - j))) & 3;
                    z += cs[base + ks + j][FT4_GRAY[tone_bits] as usize];
                }
                *v = z.norm();
            }
            let nbits = 2 * ns;
            for ib in 0..nbits {
                let mask = 1 << (nbits - 1 - ib);
                let mut one = f32::MIN;
                let mut zero = f32::MIN;
                for (i, &v) in s2[..nt].iter().enumerate() {
                    if i & mask != 0 {
                        one = one.max(v);
                    } else {
                        zero = zero.max(v);
                    }
                }
                out[(29 * g + ks) * 2 + ib] = one - zero;
            }
            ks += ns;
        }
    }
    let n = N as f32;
    let mean = out.iter().sum::<f32>() / n;
    let var = out.iter().map(|v| v * v).sum::<f32>() / n - mean * mean;
    let sd = var.max(1e-12).sqrt();
    for v in out.iter_mut() {
        *v *= scale / sd;
    }
    out
}

/// Samples of an FT4 waveform at `rate` (its ramp symbols included).
pub fn waveform_len(rate: f32) -> usize {
    NSYM * (rate / BAUD).round() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(n: usize, seed: u64, sd: f32) -> Vec<f32> {
        let mut s = seed | 1;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 11) as f64 / (1u64 << 53) as f64
        };
        (0..n)
            .map(|_| {
                let (u, v) = (next().max(1e-12), next());
                ((-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()) as f32 * sd
            })
            .collect()
    }

    fn add_signal(x: &mut [f32], text: &str, f0: f32, dt: f32, snr: f32) {
        let m = msg::pack(text).unwrap();
        let t = tones::ft4(m);
        let amp = (2.0 * 10f32.powf(snr / 10.0) * 2500.0 / 6000.0).sqrt();
        let start = ((T0 + dt) * RATE) as usize;
        let s = tones::synth(
            &t,
            BAUD as f64,
            Shape::Ft4,
            f0 as f64,
            RATE as f64,
            start,
            x.len(),
            amp,
        );
        for (a, b) in x.iter_mut().zip(s) {
            *a += b;
        }
    }

    #[test]
    fn decodes_synthetic_signals() {
        let mut x = noise(SLOT, 7, 1.0);
        add_signal(&mut x, "CQ K1ABC FN42", 1000.0, 0.0, -8.0);
        add_signal(&mut x, "K1ABC W9XYZ -07", 1500.3, 0.2, -12.0);
        add_signal(&mut x, "W9XYZ K1ABC R-12", 2200.0, -0.3, -14.0);
        let mut d = Decoder::new(Config::default());
        let got = d.decode(&x, &mut CallHashes::default());
        let texts: Vec<&str> = got.iter().map(|d| d.message.text.as_str()).collect();
        for want in ["CQ K1ABC FN42", "K1ABC W9XYZ -07", "W9XYZ K1ABC R-12"] {
            assert!(texts.contains(&want), "{want} not in {texts:?}");
        }
        let cq = got
            .iter()
            .find(|d| d.message.text == "CQ K1ABC FN42")
            .unwrap();
        assert!((cq.freq - 1000.0).abs() < 1.5, "{cq:?}");
        assert!(cq.dt.abs() < 0.03, "{cq:?}");
        assert!((cq.snr + 8.0).abs() < 3.0, "{cq:?}");
    }

    #[test]
    fn subtraction_removes_a_signal() {
        for (f0, dt) in [(1000.0f32, 0.0f32), (1333.7, 0.137), (612.1, -0.4123)] {
            let mut x = vec![0f32; SLOT];
            add_signal(&mut x, "K1ABC W9XYZ -07", f0, dt, 10.0);
            let before: f32 = x.iter().map(|v| v * v).sum();
            let dec = Decoder::new(Config::default());
            let mut spectrum = dec.fft1.forward(&x);
            let bits = msg::pack("K1ABC W9XYZ -07").unwrap();
            let d = Decode {
                message: msg::unpack(bits, &mut CallHashes::default()).unwrap(),
                bits,
                freq: f0,
                dt,
                snr: 0.0,
                sync: 0.0,
                how: 1,
                pass: 1,
                osd_distance: 0.0,
                hard_errors: 0,
                osd_second: 0.0,
            };
            dec.subtract(&mut spectrum, &d);
            let after: f32 = dec.audio(&spectrum).iter().map(|v| v * v).sum();
            let db = 10.0 * (before / after).log10();
            assert!(db > 30.0, "f0 {f0} dt {dt}: only {db:.1} dB");
        }
    }

    #[test]
    fn noise_gives_nothing() {
        let mut d = Decoder::new(Config::default());
        let mut h = CallHashes::default();
        for seed in 1..4 {
            let got = d.decode(&noise(SLOT, seed, 1.0), &mut h);
            assert!(got.is_empty(), "{got:?}");
        }
    }

    #[test]
    fn waveform_is_105_symbols() {
        assert_eq!(waveform_len(RATE), 105 * NSPS);
        let s = tones::synth_complex(&[0; FT4_SYMBOLS], BAUD as f64, Shape::Ft4, 0.0, RATE as f64);
        assert_eq!(s.len(), 105 * NSPS);
    }
}
