// SPDX-License-Identifier: GPL-3.0-or-later
//! FT8 decoder for one 15-second slot of 12 kS/s audio.
//!
//! 1. **Candidates.** A spectrogram (symbol-length windows, 1/4-symbol
//!    steps, 1/2-tone bins) is searched for the three 7×7 Costas arrays;
//!    the sync score is the power on the Costas tones against the mean of
//!    the other tones, normalised by the band's typical score.
//! 2. **Per candidate.** The slot's spectrum around the candidate is
//!    shifted to 0 Hz and transformed back at 200 S/s (32 samples per
//!    symbol). Time and frequency are refined by correlating with the
//!    Costas tones coherently, then each symbol's 8 tone amplitudes are
//!    taken with a 32-point FFT.
//! 3. **Soft bits.** Bit likelihoods from single symbols and from
//!    coherent sums over 2 and 3 symbols (which gain when the phase holds
//!    over a few symbols), each tried with belief propagation, then
//!    ordered-statistics decoding.
//! 4. **Subtraction.** Decoded signals are rebuilt and subtracted from the
//!    audio, and the search runs again for what they covered.

use crate::ldpc::{self, Bp, N};
use crate::msg::{self, CallHashes, Message};
use crate::osd;
use crate::realfft::RealFft;
use crate::tones::{self, FT8_COSTAS, FT8_GRAY, Shape};
use num_complex::Complex32 as C32;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;
use std::time::Instant;

pub const RATE: f32 = 12_000.0;
pub const NSPS: usize = 1920;
pub const BAUD: f32 = RATE / NSPS as f32;
/// Samples in a slot.
pub const SLOT: usize = 15 * 12_000;
/// Nominal start of a transmission in the slot.
pub const T0: f32 = 0.5;

const NFFT1: usize = 192_000;
const DF1: f32 = RATE / NFFT1 as f32;
const DS_RATE: f32 = 200.0;
const NFFT2: usize = 3200;
const DS_NSPS: usize = 32;

/// Frequency offsets tried in the fine sync, Hz.
const FINE_DF: [f32; 11] = [-2.5, -2.0, -1.5, -1.0, -0.5, 0.0, 0.5, 1.0, 1.5, 2.0, 2.5];

/// Raised-cosine edges of the band taken for downsampling, 101 bins.
static TAPER: std::sync::LazyLock<[f32; 101]> = std::sync::LazyLock::new(|| {
    std::array::from_fn(|k| 0.5 * (1.0 - (std::f32::consts::PI * k as f32 / 101.0).cos()))
});

/// Band subtracted around tone 0 of a decoded signal, Hz.
const SUB_BELOW: f32 = 50.0;
const SUB_ABOVE: f32 = 94.0;

const SPEC_STEP: usize = NSPS / 4;
const SPEC_NFFT: usize = 2 * NSPS;
const SPEC_DF: f32 = RATE / SPEC_NFFT as f32;

#[derive(Clone, Debug)]
pub struct Config {
    pub f_min: f32,
    pub f_max: f32,
    /// Normalised sync score a candidate needs.
    pub sync_min: f32,
    pub max_candidates: usize,
    /// Decoding passes, with subtraction between them.
    pub passes: usize,
    pub bp_iterations: usize,
    /// OSD search depth; `None` disables OSD.
    pub osd: Option<osd::Depth>,
    /// Reject OSD results that contradict more hard decisions than this.
    pub osd_max_hard_errors: usize,
    /// ... or whose soft distance is above this.
    pub osd_max_distance: f32,
    /// Largest distance ratio of an OSD result to the runner-up codeword,
    /// when no callsign in it has been heard and when one has.
    pub osd_max_ratio: f32,
    pub osd_max_ratio_known: f32,
    /// Bit LLRs are the soft metrics at unit standard deviation times this.
    pub llr_scale: f32,
    /// Log what happens to candidates within 5 Hz of this frequency.
    pub trace: Option<f32>,
    /// Keep the audio left after subtracting the decodes (`residual`).
    pub keep_residual: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            f_min: 200.0,
            f_max: 3000.0,
            sync_min: 1.5,
            max_candidates: 400,
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
            keep_residual: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Decode {
    pub message: Message,
    /// The 77 message bits.
    pub bits: u128,
    /// Audio frequency of tone 0, Hz.
    pub freq: f32,
    /// Start time relative to the nominal 0.5 s into the slot, seconds.
    pub dt: f32,
    /// SNR in 2500 Hz, dB (WSJT-X's scale).
    pub snr: f32,
    pub sync: f32,
    /// How it was decoded: 1-3 = symbols per soft-bit group; +10 if OSD.
    pub how: u8,
    pub pass: u8,
    /// For OSD decodes: soft distance, contradicted hard decisions, and
    /// the distance of the runner-up codeword.
    pub osd_distance: f32,
    pub hard_errors: usize,
    pub osd_second: f32,
}

/// Where the time went, accumulated over calls (CPU accounting, tuning).
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub slots: u64,
    /// Slots whose decoding the deadline stopped.
    pub cut_short: u64,
    pub candidates: u64,
    pub search: std::time::Duration,
    pub sync: std::time::Duration,
    pub metrics: std::time::Duration,
    pub bp: std::time::Duration,
    pub osd: std::time::Duration,
    pub subtract: std::time::Duration,
}

struct Demod {
    f0: f32,
    /// Start of the signal in the slot, seconds.
    t: f32,
    sync: f32,
    cs: [[C32; 8]; 79],
    s8: Vec<[f32; 8]>,
    nsync: usize,
}

/// Soft bits of a signal at a known place, for tools that study the
/// decoder (`examples/ft8probe.rs`).
#[doc(hidden)]
pub struct Analysis {
    pub freq: f32,
    pub dt: f32,
    pub nsync: usize,
    pub metrics: [[f32; N]; 3],
}

#[derive(Clone, Copy, Debug)]
struct Candidate {
    freq: f32,
    dt: f32,
    sync: f32,
}

pub struct Decoder {
    cfg: Config,
    fft1: RealFft,
    fft2: Arc<dyn Fft<f32>>,
    ifft2: Arc<dyn Fft<f32>>,
    spec_fft: Arc<dyn Fft<f32>>,
    fft32: Arc<dyn Fft<f32>>,
    bp: Bp,
    /// Costas reference waveforms per tone at 200 S/s.
    tone_ref: [[C32; DS_NSPS]; 8],
    /// exp(-j 2 pi df n / 200) for each fine frequency offset.
    fine_tw: [[C32; DS_NSPS]; FINE_DF.len()],
    pub stats: Stats,
    residual: Vec<f32>,
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
            residual: vec![],
        }
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// The last slot's audio without its decodes (with `keep_residual`).
    pub fn residual(&self) -> &[f32] {
        &self.residual
    }

    /// Decode a slot: `audio` starts at the slot boundary (up to 15 s;
    /// shorter is zero-padded).
    pub fn decode(&mut self, audio: &[f32], hashes: &mut CallHashes) -> Vec<Decode> {
        self.decode_until(audio, hashes, None)
    }

    /// Decode a slot, stopping at `deadline`: candidates go strongest
    /// first, and later passes only if there is time, so a short budget
    /// loses the weakest decodes.
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
        let mut spectrum = self.spectrum(&x);
        // Candidates tried so far, (freq, dt).
        let mut tried: Vec<(f32, f32)> = vec![];
        'passes: for pass in 1..=self.cfg.passes {
            if pass > 1 {
                // The audio without what earlier passes decoded.
                let t = Instant::now();
                x = self.audio(&spectrum);
                self.stats.subtract += t.elapsed();
            }
            let t = Instant::now();
            let cands = self.candidates(&x);
            self.stats.search += t.elapsed();
            self.stats.candidates += cands.len() as u64;
            let mut new: Vec<Decode> = vec![];
            for c in cands {
                if deadline.is_some_and(|d| Instant::now() >= d) {
                    self.stats.cut_short += 1;
                    found.extend(new);
                    break 'passes;
                }
                let traced = self.cfg.trace.is_some_and(|f| (f - c.freq).abs() < 5.0);
                if traced {
                    eprintln!("pass {pass} candidate {c:?}");
                }
                // In later passes only the bands that lost a signal have
                // changed; a candidate tried before elsewhere fails again.
                let near_tried = tried.iter().any(|&(f, t): &(f32, f32)| {
                    (f - c.freq).abs() < 1.6 && (t - c.dt).abs() < 0.05
                });
                let near_new = found.iter().any(|d| {
                    d.pass as usize == pass - 1
                        && c.freq > d.freq - SUB_BELOW
                        && c.freq < d.freq + SUB_ABOVE
                });
                if near_tried && !near_new {
                    continue;
                }
                tried.push((c.freq, c.dt));
                // Skip candidates sitting on a signal decoded in this pass:
                // the same signal (now subtracted) seen again. Those of
                // earlier passes may hide another signal at the same place.
                if new
                    .iter()
                    .any(|d| (d.freq - c.freq).abs() < 4.0 && (d.dt - c.dt).abs() < 0.08)
                {
                    if traced {
                        eprintln!("  on a decoded signal");
                    }
                    continue;
                }
                if let Some(mut d) = self.decode_candidate(&spectrum, c, hashes) {
                    if found.iter().chain(new.iter()).any(|e| e.bits == d.bits) {
                        continue;
                    }
                    d.pass = pass as u8;
                    // Subtract right away, so the candidates after it see
                    // the band without it.
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
        if self.cfg.keep_residual {
            self.residual = self.audio(&spectrum);
        }
        found
    }

    #[doc(hidden)]
    pub fn analyze(&mut self, audio: &[f32], freq: f32, dt: f32) -> Analysis {
        let mut x = vec![0f32; SLOT];
        let n = audio.len().min(SLOT);
        x[..n].copy_from_slice(&audio[..n]);
        let spectrum = self.spectrum(&x);
        let d = self.demod(
            &spectrum,
            Candidate {
                freq,
                dt,
                sync: 0.0,
            },
        );
        Analysis {
            freq: d.f0,
            dt: d.t - T0,
            nsync: d.nsync,
            metrics: [1, 2, 3].map(|nsym| soft_bits(&d.cs, nsym, self.cfg.llr_scale)),
        }
    }

    /// Soft bits exactly at `freq` and `dt`, without refinement.
    #[doc(hidden)]
    pub fn analyze_at(&mut self, audio: &[f32], freq: f32, dt: f32) -> Analysis {
        let mut x = vec![0f32; SLOT];
        let n = audio.len().min(SLOT);
        x[..n].copy_from_slice(&audio[..n]);
        let spectrum = self.spectrum(&x);
        let t = (dt + T0) * DS_RATE;
        let i0 = t.floor();
        let cd = self.downsample_shifted(&spectrum, freq, (t - i0) / DS_RATE);
        let (cs, _, nsync) = self.symbols(&cd, i0 as i64);
        Analysis {
            freq,
            dt,
            nsync,
            metrics: [1, 2, 3].map(|nsym| soft_bits(&cs, nsym, self.cfg.llr_scale)),
        }
    }

    /// Real audio back from the positive half of a slot spectrum.
    fn audio(&self, spectrum: &[C32]) -> Vec<f32> {
        let mut x = self.fft1.inverse(spectrum);
        x.truncate(SLOT);
        x.iter_mut().for_each(|v| *v /= NFFT1 as f32);
        x
    }

    /// Remove a decoded signal from the slot spectrum: rebuild it at
    /// 200 S/s, take its slowly varying complex amplitude from the
    /// received samples, and subtract its spectrum.
    fn subtract(&self, spectrum: &mut [C32], d: &Decode) {
        let t = (d.dt + T0) * DS_RATE;
        let i0 = t.floor();
        let frac = (t - i0) / DS_RATE;
        let i0 = i0 as i64;
        let cd = self.downsample_shifted(spectrum, d.freq, frac);
        let reference = tones::synth_complex(
            &tones::ft8(d.bits),
            BAUD as f64,
            Shape::Ft8,
            0.0,
            DS_RATE as f64,
        );
        let at = |n: usize| (i0 + n as i64).rem_euclid(NFFT2 as i64) as usize;
        let camp: Vec<C32> = reference
            .iter()
            .enumerate()
            .map(|(n, r)| cd[at(n)] * r.conj())
            .collect();
        // Two 33-sample moving averages: a triangle 1/3 s wide.
        let amp = boxcar(&boxcar(&camp, 33), 33);
        let mut s = vec![C32::default(); NFFT2];
        for (n, (a, r)) in amp.iter().zip(&reference).enumerate() {
            s[at(n)] = a * r;
        }
        self.fft2.process(&mut s);
        // Back to the unshifted time base, and to the spectrum's scale
        // (downsampling scaled by 1 / sqrt(NFFT1 NFFT2)).
        let scale = ((NFFT1 * NFFT2) as f32).sqrt() / NFFT2 as f32;
        // The model covers -100..100 Hz around tone 0 at 200 S/s; take
        // the GFSK sidelobes too (about 1 % of the power is outside the
        // band the amplitude was measured in).
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

    fn spectrum(&self, x: &[f32]) -> Vec<C32> {
        self.fft1.forward(x)
    }

    /// Coarse search on a spectrogram.
    fn candidates(&self, x: &[f32]) -> Vec<Candidate> {
        let nsteps = SLOT / SPEC_STEP - 3;
        let i_min = (self.cfg.f_min / SPEC_DF) as usize;
        let i_max = ((self.cfg.f_max / SPEC_DF) as usize).min(SPEC_NFFT / 2 - 16);
        let nbins = i_max + 16;
        // Power spectra of symbol-length frames, two real frames per
        // complex FFT (one as the real part, one as the imaginary).
        let mut s = vec![0f32; nsteps * nbins];
        let mut buf = vec![C32::default(); SPEC_NFFT];
        for j in (0..nsteps).step_by(2) {
            let j2 = (j + 1).min(nsteps - 1);
            for (k, b) in buf.iter_mut().enumerate() {
                *b = if k < NSPS {
                    C32::new(x[j * SPEC_STEP + k], x[j2 * SPEC_STEP + k])
                } else {
                    C32::default()
                };
            }
            self.spec_fft.process(&mut buf);
            for i in 0..nbins {
                let z = buf[i];
                let zc = buf[(SPEC_NFFT - i) % SPEC_NFFT].conj();
                s[j * nbins + i] = (z + zc).norm_sqr() * 0.25;
                if j2 != j {
                    s[j2 * nbins + i] = (z - zc).norm_sqr() * 0.25;
                }
            }
        }
        // Sum of the 7 tones from each bin (the sync score's reference).
        let mut tsum = vec![0f32; nsteps * nbins];
        for j in 0..nsteps {
            let row = &s[j * nbins..(j + 1) * nbins];
            for i in 0..nbins - 14 {
                tsum[j * nbins + i] = (0..7).map(|t| row[i + 2 * t]).sum();
            }
        }
        // Lags in steps from the nominal start: 0.04 s each, ±2.5 s. For
        // each lag, the three Costas arrays' power and reference sums go
        // into per-bin accumulators.
        let j0 = (T0 / (SPEC_STEP as f32 / RATE)).round() as i32;
        let ni = i_max - i_min;
        let mut best = vec![(0f32, 0i32, 0f32, 0i32); ni]; // (sync, lag, sync near, lag near)
        let mut ta = vec![[0f32; 3]; ni];
        let mut t0 = vec![[0f32; 3]; ni];
        for lag in -62..=62i32 {
            ta.iter_mut().for_each(|v| *v = [0.0; 3]);
            t0.iter_mut().for_each(|v| *v = [0.0; 3]);
            for a in 0..3 {
                for (k, &tone) in FT8_COSTAS.iter().enumerate() {
                    let j = j0 + lag + 4 * (36 * a as i32 + k as i32);
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
                let score = |ta: f32, t0: f32| if t0 > ta { ta / ((t0 - ta) / 6.0) } else { 0.0 };
                let abc = score(ta[0] + ta[1] + ta[2], t0[0] + t0[1] + t0[2]);
                let bc = score(ta[1] + ta[2], t0[1] + t0[2]);
                let sync = abc.max(bc);
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
        // Normalise by the 40th percentile of the best scores.
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
                        dt: lag as f32 * SPEC_STEP as f32 / RATE,
                        sync,
                    });
                }
            }
        }
        cands.sort_by(|a, b| b.sync.total_cmp(&a.sync));
        // Keep the strongest of near duplicates.
        let mut kept: Vec<Candidate> = vec![];
        for c in cands {
            if kept
                .iter()
                .any(|k| (k.freq - c.freq).abs() < 4.0 && (k.dt - c.dt).abs() < 0.04 + 1e-3)
            {
                continue;
            }
            kept.push(c);
            if kept.len() >= self.cfg.max_candidates {
                break;
            }
        }
        kept
    }

    /// The slot around `f0` at 200 S/s, tone 0 at 0 Hz.
    fn downsample(&self, spectrum: &[C32], f0: f32) -> Vec<C32> {
        self.downsample_shifted(spectrum, f0, 0.0)
    }

    /// Same, advanced by `shift` seconds (sample n is at n / 200 + shift).
    fn downsample_shifted(&self, spectrum: &[C32], f0: f32, shift: f32) -> Vec<C32> {
        let i0 = (f0 / DF1).round() as i64;
        let ib = (i0 - (1.5 * BAUD / DF1) as i64).max(1);
        let it = (i0 + (8.5 * BAUD / DF1) as i64).min(spectrum.len() as i64 - 1);
        let mut c = vec![C32::default(); NFFT2];
        let nb = (it - ib + 1) as usize;
        let taper = TAPER.len().min(nb / 2);
        // The time shift is a phase ramp over the bins: a rotating phasor.
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

    /// Coherent Costas correlation (200 S/s): the power of each sync
    /// symbol's correlation with its tone, summed, at `n` consecutive
    /// start samples from `first`. Each
    /// Costas symbol's correlation is one DFT bin of a 32-sample window,
    /// so a step of the window updates it with a sliding DFT:
    /// X(s + 1) = (X(s) - x(s) + x(s + 32)) e^(j 2 pi tone / 32).
    fn sync_lags(&self, cd: &[C32], first: i64, n: usize) -> Vec<f32> {
        let at = |i: i64| {
            if i >= 0 && (i as usize) < cd.len() {
                cd[i as usize]
            } else {
                C32::default()
            }
        };
        let mut sum = vec![0f32; n];
        for a in 0..3 {
            for (k, &tone) in FT8_COSTAS.iter().enumerate() {
                let r = &self.tone_ref[tone as usize];
                let step = r[1];
                let start = first + (DS_NSPS * (36 * a + k)) as i64;
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

    /// The same at start sample `i0` for each of the frequency offsets in
    /// `FINE_DF`. An offset only rotates the products within a symbol (a
    /// common phase per symbol doesn't change the power), so a table of
    /// 32 rotations per offset does it.
    fn sync_fine_df(&self, cd: &[C32], i0: i64) -> [f32; FINE_DF.len()] {
        let mut sum = [0f32; FINE_DF.len()];
        let mut p = [C32::default(); DS_NSPS];
        for a in 0..3 {
            for (k, &tone) in FT8_COSTAS.iter().enumerate() {
                let start = i0 + (DS_NSPS * (36 * a + k)) as i64;
                for (n, (pn, r)) in p.iter_mut().zip(&self.tone_ref[tone as usize]).enumerate() {
                    let idx = start + n as i64;
                    *pn = if idx >= 0 && (idx as usize) < cd.len() {
                        cd[idx as usize] * r.conj()
                    } else {
                        C32::default()
                    };
                }
                for (s, t) in sum.iter_mut().zip(&self.fine_tw) {
                    *s += p.iter().zip(t).map(|(a, b)| a * b).sum::<C32>().norm_sqr();
                }
            }
        }
        sum
    }

    /// Each symbol's 8 tone amplitudes for a signal starting at sample
    /// `i0`, and how many Costas symbols have their tone on top.
    fn symbols(&self, cd: &[C32], i0: i64) -> ([[C32; 8]; 79], Vec<[f32; 8]>, usize) {
        let mut cs = [[C32::default(); 8]; 79];
        let mut buf = [C32::default(); DS_NSPS];
        for (k, row) in cs.iter_mut().enumerate() {
            let start = i0 + (k * DS_NSPS) as i64;
            for (n, b) in buf.iter_mut().enumerate() {
                let idx = start + n as i64;
                *b = if idx >= 0 && (idx as usize) < cd.len() {
                    cd[idx as usize]
                } else {
                    C32::default()
                };
            }
            self.fft32.process(&mut buf);
            row.copy_from_slice(&buf[..8]);
        }
        let s8: Vec<[f32; 8]> = cs
            .iter()
            .map(|r| std::array::from_fn(|t| r[t].norm()))
            .collect();

        // Hard sync check: how many Costas symbols have their tone on top.
        let mut nsync = 0;
        for a in 0..3 {
            for k in 0..7 {
                let row = &s8[36 * a + k];
                let top = (0..8).max_by(|&x, &y| row[x].total_cmp(&row[y])).unwrap();
                if top == FT8_COSTAS[k] as usize {
                    nsync += 1;
                }
            }
        }
        (cs, s8, nsync)
    }

    /// Refine a candidate and take its symbols' tone amplitudes.
    fn demod(&mut self, spectrum: &[C32], c: Candidate) -> Demod {
        let t = Instant::now();
        let mut f0 = c.freq;
        let mut cd = self.downsample(spectrum, f0);
        let nominal = ((c.dt + T0) * DS_RATE).round() as i64;
        // The peak of sync over start samples first..first + len (the
        // first of equal ones), and the values around it.
        let peak = |v: &[f32], first: i64| {
            let mut best = (first, 0f32);
            for (k, &s) in v.iter().enumerate() {
                if s > best.1 {
                    best = (first + k as i64, s);
                }
            }
            best
        };
        let mut lags = (nominal - 11, self.sync_lags(&cd, nominal - 11, 23));
        let (mut i0, mut best) = peak(&lags.1[1..22], nominal - 10);
        // Frequency in 0.5 Hz steps, then a parabola through the peak.
        let fs = self.sync_fine_df(&cd, i0);
        let kb = (0..fs.len())
            .max_by(|&a, &b| fs[a].total_cmp(&fs[b]))
            .unwrap();
        let mut df = (kb as f32 - 5.0) * 0.5;
        if kb > 0 && kb < fs.len() - 1 {
            df += 0.5 * parabola_peak(fs[kb - 1], fs[kb], fs[kb + 1]);
        }
        if df.abs() > 0.05 {
            f0 += df;
            // A shift this small (under 3 Hz of a 62 Hz band) is a phase
            // rotation of the samples: on the A9 an inverse FFT less.
            rotate(&mut cd, -df / DS_RATE);
            let start = i0;
            lags = (start - 5, self.sync_lags(&cd, start - 5, 11));
            (i0, best) = peak(&lags.1[1..10], start - 4);
        }
        // Time to a fraction of a sample (the neighbours are in `lags`).
        let near = |i: i64| lags.1[(i - lags.0) as usize];
        let p = parabola_peak(near(i0 - 1), best, near(i0 + 1));
        let frac = p.clamp(-0.5, 0.5);
        if frac.abs() > 0.05 {
            cd = delay(&cd, frac);
        }

        self.stats.sync += t.elapsed();
        let t = Instant::now();
        let (cs, s8, nsync) = self.symbols(&cd, i0);
        let traced = self.cfg.trace.is_some_and(|f| (f - c.freq).abs() < 5.0);
        if traced {
            let hard: String = s8
                .iter()
                .map(|r| {
                    (b'0' + (0..8).max_by(|&x, &y| r[x].total_cmp(&r[y])).unwrap() as u8) as char
                })
                .collect();
            eprintln!("  fine: f0 {f0:.2} i0 {i0} sync {best:.3e} nsync {nsync}\n  hard {hard}");
        }
        self.stats.metrics += t.elapsed();
        Demod {
            f0,
            t: (i0 as f32 + frac) / DS_RATE,
            sync: best,
            cs,
            s8,
            nsync,
        }
    }

    fn decode_candidate(
        &mut self,
        spectrum: &[C32],
        c: Candidate,
        hashes: &mut CallHashes,
    ) -> Option<Decode> {
        let traced = self.cfg.trace.is_some_and(|f| (f - c.freq).abs() < 5.0);
        let Demod {
            f0,
            t: t_start,
            sync: best,
            cs,
            s8,
            nsync,
        } = self.demod(spectrum, c);
        if nsync <= 6 {
            return None;
        }
        let t = Instant::now();

        let metrics: [[f32; N]; 3] = [1, 2, 3].map(|nsym| soft_bits(&cs, nsym, self.cfg.llr_scale));
        self.stats.metrics += t.elapsed();
        let t = Instant::now();
        for (m, how) in metrics.iter().zip([1u8, 2, 3]) {
            let d = self.bp.decode(m, self.cfg.bp_iterations);
            if d.errors == 0
                && let Some(r) = self.accept(&d.codeword, None, &s8, f0, t_start, best, how, hashes)
            {
                self.stats.bp += t.elapsed();
                return Some(r);
            }
        }
        self.stats.bp += t.elapsed();
        let t = Instant::now();
        let mut result = None;
        if let Some(depth) = self.cfg.osd {
            'osd: for (m, how) in metrics.iter().zip([1u8, 2, 3]) {
                if let Some(f) = osd::decode(m, depth) {
                    if traced {
                        let text = crate::crc::check(ldpc::message(&f.codeword))
                            .and_then(|b| msg::unpack(b, &mut CallHashes::default()))
                            .map(|m| m.text);
                        eprintln!(
                            "  osd nsym {how}: hard errors {} distance {:.1} {text:?}",
                            f.hard_errors, f.distance
                        );
                    }
                    {
                        result = self
                            .accept(
                                &f.codeword,
                                Some(&f),
                                &s8,
                                f0,
                                t_start,
                                best,
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
                            break 'osd;
                        }
                    }
                }
            }
        }
        self.stats.osd += t.elapsed();
        if traced {
            eprintln!(
                "  result {:?}",
                result.as_ref().map(|d: &Decode| &d.message.text)
            );
        }
        result
    }

    /// OSD finds a CRC-valid codeword near almost anything if it searches
    /// enough, so its results must also look like real traffic: a
    /// standard message with plausible callsigns, close to the received
    /// bits. With a callsign heard before, it may be further away.
    fn osd_plausible(&self, m: &Message, f: &osd::Found, hashes: &CallHashes) -> bool {
        let c = &self.cfg;
        osd_plausible(
            m,
            f,
            hashes,
            c.osd_max_hard_errors,
            c.osd_max_distance,
            c.osd_max_ratio,
            c.osd_max_ratio_known,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn accept(
        &self,
        cw: &ldpc::Bits,
        osd: Option<&osd::Found>,
        s8: &[[f32; 8]],
        f0: f32,
        t_start: f32,
        sync: f32,
        how: u8,
        hashes: &mut CallHashes,
    ) -> Option<Decode> {
        let a91 = ldpc::message(cw);
        let bits = crate::crc::check(a91)?;
        if bits == 0 {
            return None;
        }
        let message = msg::peek(bits, hashes)?;
        if let Some(f) = osd
            && !self.osd_plausible(&message, f, hashes)
        {
            return None;
        }
        for c in &message.calls {
            hashes.save(c);
        }
        let tones = tones::ft8(bits);
        let mut xsig = 0.0;
        let mut xnoi = 0.0;
        for (k, &t) in tones.iter().enumerate() {
            xsig += s8[k][t as usize].powi(2);
            xnoi += s8[k][(t as usize + 4) % 8].powi(2);
        }
        let ratio = (xsig / xnoi.max(1e-12) - 1.0).max(1e-3);
        let snr = (10.0 * ratio.log10() - 27.0).max(-24.0);
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

/// See `Decoder::osd_plausible`; shared with FT4.
pub(crate) fn osd_plausible(
    m: &Message,
    f: &osd::Found,
    hashes: &CallHashes,
    max_hard_errors: usize,
    max_distance: f32,
    max_ratio: f32,
    max_ratio_known: f32,
) -> bool {
    if f.hard_errors > max_hard_errors || f.distance > max_distance {
        return false;
    }
    if m.i3 != 1
        || !m
            .calls
            .iter()
            .all(|c| msg::plausible_call(c) && !c.ends_with("/R"))
    {
        return false;
    }
    // A real signal's codeword is much closer than the chance ones that
    // pass the CRC; a false decode is just the best of those.
    let ratio = if f.second.is_finite() {
        f.distance / f.second
    } else {
        0.0
    };
    let known = m.calls.iter().any(|c| hashes.knows(c));
    ratio <= if known { max_ratio_known } else { max_ratio }
}

/// Offset (in steps, -0.5..0.5 when `b` is the largest) of the peak of
/// a parabola through three equally spaced values.
pub(crate) fn parabola_peak(a: f32, b: f32, c: f32) -> f32 {
    let d = a - 2.0 * b + c;
    if d >= 0.0 { 0.0 } else { 0.5 * (a - c) / d }
}

/// Bit log-likelihoods from coherent sums over groups of `nsym` symbols.
fn soft_bits(cs: &[[C32; 8]; 79], nsym: usize, scale: f32) -> [f32; N] {
    let mut out = [0f32; N];
    for half in 0..2 {
        let base = if half == 0 { 7 } else { 43 };
        let mut ks = 0;
        while ks < 29 {
            let ns = nsym.min(29 - ks);
            let nt = 1usize << (3 * ns);
            // Squared magnitudes: the maxima are the same, and only the
            // winners need a square root (512 of them for 3 symbols).
            // The sums over the group's symbols for every tone sequence,
            // the first symbol's tone in the top bits: built a symbol at a
            // time, sharing the partial sums.
            let mut z = [C32::default(); 512];
            let mut len = 1;
            for j in 0..ns {
                let row = &cs[base + ks + j];
                for i in (0..len).rev() {
                    let p = z[i];
                    for (t, &g) in FT8_GRAY.iter().enumerate() {
                        z[i * 8 + t] = p + row[g as usize];
                    }
                }
                len *= 8;
            }
            let mut s2 = [0f32; 512];
            for (v, z) in s2[..nt].iter_mut().zip(&z) {
                *v = z.norm_sqr();
            }
            let nbits = 3 * ns;
            for ib in 0..nbits {
                let mask = 1 << (nbits - 1 - ib);
                let mut one = 0f32;
                let mut zero = 0f32;
                for (i, &v) in s2[..nt].iter().enumerate() {
                    if i & mask != 0 {
                        one = one.max(v);
                    } else {
                        zero = zero.max(v);
                    }
                }
                out[(29 * half + ks) * 3 + ib] = one.sqrt() - zero.sqrt();
            }
            ks += ns;
        }
    }
    // Unit standard deviation, then the LLR scale.
    let n = N as f32;
    let mean = out.iter().sum::<f32>() / n;
    let var = out.iter().map(|v| v * v).sum::<f32>() / n - mean * mean;
    let sd = var.max(1e-12).sqrt();
    for v in out.iter_mut() {
        *v *= scale / sd;
    }
    out
}

/// Turn `v` by `cycles` per sample (a frequency shift).
fn rotate(v: &mut [C32], cycles: f32) {
    let step = 2.0 * std::f32::consts::PI * cycles;
    let rot = C32::new(step.cos(), step.sin());
    let mut ph = C32::new(1.0, 0.0);
    for x in v.iter_mut() {
        *x *= ph;
        ph *= rot;
    }
}

/// Samples at n + `frac` (|frac| <= 0.5) by an 8-tap windowed sinc. The
/// signal is within a quarter of the sample rate (0..53 Hz at 200 S/s),
/// where its error is far below the noise of a decodable signal.
fn delay(v: &[C32], frac: f32) -> Vec<C32> {
    use std::f32::consts::PI;
    // taps for samples n - 3 ..= n + 4
    let h: [f32; 8] = std::array::from_fn(|k| {
        let t = frac - (k as f32 - 3.0);
        let sinc = if t.abs() < 1e-6 {
            1.0
        } else {
            (PI * t).sin() / (PI * t)
        };
        // Blackman over ±4 samples
        let x = (t + 4.0) / 8.0;
        let w = 0.42 - 0.5 * (2.0 * PI * x).cos() + 0.08 * (4.0 * PI * x).cos();
        sinc * w
    });
    (0..v.len())
        .map(|n| {
            let mut acc = C32::default();
            for (k, &hk) in h.iter().enumerate() {
                let i = n as i64 + k as i64 - 3;
                if i >= 0 && (i as usize) < v.len() {
                    acc += v[i as usize] * hk;
                }
            }
            acc
        })
        .collect()
}

/// Centred moving average of width `w`, averaging only over the samples
/// that exist near the ends.
pub(crate) fn boxcar(v: &[C32], w: usize) -> Vec<C32> {
    let mut acc = Vec::with_capacity(v.len() + 1);
    let mut sum = C32::default();
    acc.push(sum);
    for &x in v {
        sum += x;
        acc.push(sum);
    }
    let h = w / 2;
    (0..v.len())
        .map(|i| {
            let lo = i.saturating_sub(h);
            let hi = (i + w - h).min(v.len());
            (acc[hi] - acc[lo]) / (hi - lo) as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(n: usize, seed: u64, sd: f32) -> Vec<f32> {
        // xorshift + Box-Muller
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

    /// Signal at `snr` dB in 2500 Hz against unit-variance noise.
    fn add_signal(x: &mut [f32], text: &str, f0: f32, dt: f32, snr: f32) {
        let m = msg::pack(text).unwrap();
        let t = tones::ft8(m);
        // Noise power in 2500 Hz = 2500/6000 of the total (unit variance).
        let amp = (2.0 * 10f32.powf(snr / 10.0) * 2500.0 / 6000.0).sqrt();
        let start = ((T0 + dt) * RATE) as usize;
        let s = tones::synth(
            &t,
            BAUD as f64,
            Shape::Ft8,
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
        add_signal(&mut x, "CQ K1ABC FN42", 1000.0, 0.0, -10.0);
        add_signal(&mut x, "K1ABC W9XYZ -07", 1500.3, 0.3, -15.0);
        add_signal(&mut x, "W9XYZ K1ABC R-12", 2200.0, -0.4, -18.0);
        let mut d = Decoder::new(Config::default());
        let mut h = CallHashes::default();
        let got = d.decode(&x, &mut h);
        let texts: Vec<&str> = got.iter().map(|d| d.message.text.as_str()).collect();
        for want in ["CQ K1ABC FN42", "K1ABC W9XYZ -07", "W9XYZ K1ABC R-12"] {
            assert!(texts.contains(&want), "{want} not in {texts:?}");
        }
        let cq = got
            .iter()
            .find(|d| d.message.text == "CQ K1ABC FN42")
            .unwrap();
        assert!((cq.freq - 1000.0).abs() < 1.0, "{cq:?}");
        assert!(cq.dt.abs() < 0.05, "{cq:?}");
        assert!((cq.snr + 10.0).abs() < 3.0, "{cq:?}");
    }

    /// A decoded signal subtracts away: noise-free, odd frequency and a
    /// start between samples.
    #[test]
    fn subtraction_removes_a_signal() {
        for (f0, dt) in [
            (1000.0f32, 0.0f32),
            (1000.3, 0.137),
            (1733.7, -0.4123),
            (612.1, 1.9017),
        ] {
            let mut x = vec![0f32; SLOT];
            add_signal(&mut x, "K1ABC W9XYZ -07", f0, dt, 10.0);
            let before: f32 = x.iter().map(|v| v * v).sum();
            let dec = Decoder::new(Config::default());
            let mut spectrum = dec.spectrum(&x);
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

    /// The fractional delay matches a tone's true samples, and the
    /// rotation shifts it.
    #[test]
    fn delay_and_rotate_a_tone() {
        let f = 40.0 / DS_RATE; // the top of the band, cycles per sample
        let tone = |t: f32| C32::from_polar(1.0, 2.0 * std::f32::consts::PI * f * t);
        let v: Vec<C32> = (0..200).map(|n| tone(n as f32)).collect();
        for frac in [-0.5f32, -0.23, 0.31, 0.5] {
            let d = delay(&v, frac);
            for (n, &x) in d.iter().enumerate().take(190).skip(10) {
                let e = (x - tone(n as f32 + frac)).norm();
                assert!(e < 4e-3, "frac {frac} n {n}: error {e}");
            }
        }
        let mut r = v.clone();
        rotate(&mut r, -f);
        assert!(r.iter().all(|x| (x - C32::new(1.0, 0.0)).norm() < 1e-3));
    }

    #[test]
    fn deadline_stops_decoding() {
        let mut x = noise(SLOT, 3, 1.0);
        add_signal(&mut x, "K1ABC W9XYZ -07", 1500.0, 0.0, -5.0);
        let mut d = Decoder::new(Config::default());
        let got = d.decode_until(&x, &mut CallHashes::default(), Some(Instant::now()));
        assert!(got.is_empty());
        assert_eq!(d.stats.cut_short, 1);
        assert_eq!(d.decode(&x, &mut CallHashes::default()).len(), 1);
    }

    #[test]
    fn noise_gives_nothing() {
        let mut d = Decoder::new(Config::default());
        let mut h = CallHashes::default();
        for seed in 1..4 {
            let x = noise(SLOT, seed, 1.0);
            let got = d.decode(&x, &mut h);
            assert!(got.is_empty(), "{got:?}");
        }
    }
}
