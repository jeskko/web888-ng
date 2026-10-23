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
use crate::tones::{self, FT8_COSTAS, FT8_GRAY};
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
    /// The same limits when no callsign in the message has been heard.
    pub osd_max_hard_errors_unknown: usize,
    pub osd_max_distance_unknown: f32,
    /// Bit LLRs are the soft metrics at unit standard deviation times this.
    pub llr_scale: f32,
    /// Log what happens to candidates within 5 Hz of this frequency.
    pub trace: Option<f32>,
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
                order2: 60,
            }),
            osd_max_hard_errors: 44,
            osd_max_distance: 70.0,
            osd_max_hard_errors_unknown: 32,
            osd_max_distance_unknown: 45.0,
            llr_scale: 5.5,
            trace: None,
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
    /// For OSD decodes: soft distance and contradicted hard decisions.
    pub osd_distance: f32,
    pub hard_errors: usize,
}

/// Where the time went, accumulated over calls (CPU accounting, tuning).
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub slots: u64,
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
    fft1: Arc<dyn Fft<f32>>,
    ifft2: Arc<dyn Fft<f32>>,
    spec_fft: Arc<dyn Fft<f32>>,
    fft32: Arc<dyn Fft<f32>>,
    bp: Bp,
    /// Costas reference waveforms per tone at 200 S/s.
    tone_ref: [[C32; DS_NSPS]; 8],
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
            fft1: p.plan_fft_forward(NFFT1),
            ifft2: p.plan_fft_inverse(NFFT2),
            spec_fft: p.plan_fft_forward(SPEC_NFFT),
            fft32: p.plan_fft_forward(DS_NSPS),
            bp: Bp::default(),
            tone_ref,
            stats: Stats::default(),
        }
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// Decode a slot: `audio` starts at the slot boundary (up to 15 s;
    /// shorter is zero-padded).
    pub fn decode(&mut self, audio: &[f32], hashes: &mut CallHashes) -> Vec<Decode> {
        let mut x = vec![0f32; SLOT];
        let n = audio.len().min(SLOT);
        x[..n].copy_from_slice(&audio[..n]);
        let mut found: Vec<Decode> = vec![];
        self.stats.slots += 1;
        for pass in 1..=self.cfg.passes {
            let t = Instant::now();
            let cands = self.candidates(&x);
            let spectrum = self.spectrum(&x);
            self.stats.search += t.elapsed();
            self.stats.candidates += cands.len() as u64;
            let mut new = vec![];
            for c in cands {
                let traced = self.cfg.trace.is_some_and(|f| (f - c.freq).abs() < 5.0);
                if traced {
                    eprintln!("pass {pass} candidate {c:?}");
                }
                // Skip candidates sitting on a signal decoded in this pass;
                // those of earlier passes are subtracted, and what is left
                // there may be another signal.
                if new
                    .iter()
                    .any(|d: &Decode| (d.freq - c.freq).abs() < 4.0 && (d.dt - c.dt).abs() < 0.08)
                {
                    if traced {
                        eprintln!("  on a decoded signal");
                    }
                    continue;
                }
                if let Some(mut d) = self.decode_candidate(&spectrum, c, hashes) {
                    if found
                        .iter()
                        .chain(new.iter())
                        .any(|e: &Decode| e.bits == d.bits)
                    {
                        continue;
                    }
                    d.pass = pass as u8;
                    new.push(d);
                }
            }
            if new.is_empty() {
                break;
            }
            let t = Instant::now();
            for d in &new {
                subtract(&mut x, d);
            }
            self.stats.subtract += t.elapsed();
            found.extend(new);
        }
        if let Ok(path) = std::env::var("FTX_RESIDUAL") {
            let _ = crate::wav::write(path.as_ref(), &x, RATE as u32);
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

    fn spectrum(&self, x: &[f32]) -> Vec<C32> {
        let mut buf: Vec<C32> = (0..NFFT1)
            .map(|i| C32::new(*x.get(i).unwrap_or(&0.0), 0.0))
            .collect();
        self.fft1.process(&mut buf);
        buf.truncate(NFFT1 / 2);
        buf
    }

    /// Coarse search on a spectrogram.
    fn candidates(&self, x: &[f32]) -> Vec<Candidate> {
        let nsteps = SLOT / SPEC_STEP - 3;
        let i_min = (self.cfg.f_min / SPEC_DF) as usize;
        let i_max = ((self.cfg.f_max / SPEC_DF) as usize).min(SPEC_NFFT / 2 - 16);
        let nbins = i_max + 16;
        let mut s = vec![0f32; nsteps * nbins];
        let mut buf = vec![C32::default(); SPEC_NFFT];
        for j in 0..nsteps {
            for (k, b) in buf.iter_mut().enumerate() {
                *b = if k < NSPS {
                    C32::new(x[j * SPEC_STEP + k], 0.0)
                } else {
                    C32::default()
                };
            }
            self.spec_fft.process(&mut buf);
            for i in 0..nbins {
                s[j * nbins + i] = buf[i].norm_sqr();
            }
        }
        // Lags in steps from the nominal start: 0.04 s each, ±2.5 s.
        let j0 = (T0 / (SPEC_STEP as f32 / RATE)).round() as i32;
        let lags = -62..=62i32;
        let at = |j: i32, i: usize| -> Option<f32> {
            (j >= 0 && (j as usize) < nsteps).then(|| s[j as usize * nbins + i])
        };
        let mut best: Vec<(f32, i32, f32, i32)> = Vec::with_capacity(i_max - i_min); // (sync, lag, sync near, lag near)
        for i in i_min..i_max {
            let mut b = (0f32, 0i32);
            let mut bn = (0f32, 0i32);
            for lag in lags.clone() {
                let mut ta = [0f32; 3];
                let mut t0 = [0f32; 3];
                for a in 0..3 {
                    for (k, &tone) in FT8_COSTAS.iter().enumerate() {
                        let j = j0 + lag + 4 * (36 * a as i32 + k as i32);
                        let Some(v) = at(j, i + 2 * tone as usize) else {
                            continue;
                        };
                        ta[a] += v;
                        for t in 0..7 {
                            t0[a] += at(j, i + 2 * t).unwrap_or(0.0);
                        }
                    }
                }
                let score = |ta: f32, t0: f32| if t0 > ta { ta / ((t0 - ta) / 6.0) } else { 0.0 };
                let abc = score(ta.iter().sum(), t0.iter().sum());
                let bc = score(ta[1] + ta[2], t0[1] + t0[2]);
                let sync = abc.max(bc);
                if sync > b.0 {
                    b = (sync, lag);
                }
                if lag.abs() <= 10 && sync > bn.0 {
                    bn = (sync, lag);
                }
            }
            best.push((b.0, b.1, bn.0, bn.1));
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
        let taper = 101.min(nb / 2);
        for k in 0..nb {
            let mut v = spectrum[(ib + k as i64) as usize];
            let w = if k < taper {
                0.5 * (1.0 - (std::f32::consts::PI * k as f32 / taper as f32).cos())
            } else if k >= nb - taper {
                0.5 * (1.0 - (std::f32::consts::PI * (nb - 1 - k) as f32 / taper as f32).cos())
            } else {
                1.0
            };
            v *= w;
            if shift != 0.0 {
                let ph = 2.0 * std::f32::consts::PI * (ib + k as i64 - i0) as f32 * DF1 * shift;
                v *= C32::new(ph.cos(), ph.sin());
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

    /// Coherent Costas correlation at start sample `i0` (200 S/s), with
    /// the frequency offset `df` Hz removed.
    fn sync_fine(&self, cd: &[C32], i0: i64, df: f32) -> f32 {
        let w = -2.0 * std::f32::consts::PI * df / DS_RATE;
        let mut sum = 0.0;
        for a in 0..3 {
            for (k, &tone) in FT8_COSTAS.iter().enumerate() {
                let start = i0 + (DS_NSPS * (36 * a + k)) as i64;
                let mut z = C32::default();
                for (n, r) in self.tone_ref[tone as usize].iter().enumerate() {
                    let idx = start + n as i64;
                    if idx < 0 || idx as usize >= cd.len() {
                        continue;
                    }
                    let ph = w * idx as f32;
                    z += cd[idx as usize] * r.conj() * C32::new(ph.cos(), ph.sin());
                }
                sum += z.norm_sqr();
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
        let mut i0 = nominal;
        let mut best = 0.0;
        for di in -10..=10 {
            let s = self.sync_fine(&cd, nominal + di, 0.0);
            if s > best {
                best = s;
                i0 = nominal + di;
            }
        }
        // Frequency in 0.5 Hz steps, then a parabola through the peak.
        let fs: Vec<f32> = (-5..=5)
            .map(|k| self.sync_fine(&cd, i0, k as f32 * 0.5))
            .collect();
        let kb = (0..fs.len())
            .max_by(|&a, &b| fs[a].total_cmp(&fs[b]))
            .unwrap();
        let mut df = (kb as f32 - 5.0) * 0.5;
        if kb > 0 && kb < fs.len() - 1 {
            df += 0.5 * parabola_peak(fs[kb - 1], fs[kb], fs[kb + 1]);
        }
        if df.abs() > 0.05 {
            f0 += df;
            cd = self.downsample(spectrum, f0);
            let start = i0;
            best = 0.0;
            for di in -4..=4 {
                let s = self.sync_fine(&cd, start + di, 0.0);
                if s > best {
                    best = s;
                    i0 = start + di;
                }
            }
        }
        // Time to a fraction of a sample.
        let p = parabola_peak(
            self.sync_fine(&cd, i0 - 1, 0.0),
            best,
            self.sync_fine(&cd, i0 + 1, 0.0),
        );
        let frac = p.clamp(-0.5, 0.5);
        if frac.abs() > 0.05 {
            cd = self.downsample_shifted(spectrum, f0, frac / DS_RATE);
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
        if f.hard_errors > self.cfg.osd_max_hard_errors || f.distance > self.cfg.osd_max_distance {
            return false;
        }
        if m.i3 != 1 || !m.calls.iter().all(|c| msg::plausible_call(c)) {
            return false;
        }
        let known = m.calls.iter().any(|c| hashes.knows(c));
        known
            || (f.distance <= self.cfg.osd_max_distance_unknown
                && f.hard_errors <= self.cfg.osd_max_hard_errors_unknown)
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
        })
    }
}

/// Offset (in steps, -0.5..0.5 when `b` is the largest) of the peak of
/// a parabola through three equally spaced values.
fn parabola_peak(a: f32, b: f32, c: f32) -> f32 {
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
            let mut s2 = vec![0f32; nt];
            for (i, v) in s2.iter_mut().enumerate() {
                let mut z = C32::default();
                for j in 0..ns {
                    let tone_bits = (i >> (3 * (ns - 1 - j))) & 7;
                    z += cs[base + ks + j][FT8_GRAY[tone_bits] as usize];
                }
                *v = z.norm();
            }
            let nbits = 3 * ns;
            for ib in 0..nbits {
                let mask = 1 << (nbits - 1 - ib);
                let mut one = f32::MIN;
                let mut zero = f32::MIN;
                for (i, &v) in s2.iter().enumerate() {
                    if i & mask != 0 {
                        one = one.max(v);
                    } else {
                        zero = zero.max(v);
                    }
                }
                out[(29 * half + ks) * 3 + ib] = one - zero;
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

/// Subtract a decoded signal from the audio: rebuild its waveform,
/// estimate the slowly varying complex amplitude, remove it.
fn subtract(x: &mut [f32], d: &Decode) {
    let tones = tones::ft8(d.bits);
    let start = ((d.dt + T0) * RATE).round() as i64;
    let reference = reference_wave(&tones, d.freq);
    // camp = x * conj(ref) holds the amplitude at 0 Hz (and an image at
    // twice the signal frequency, which the smoothing removes).
    let n = reference.len();
    let mut camp = vec![C32::default(); n];
    for (i, r) in reference.iter().enumerate() {
        let k = start + i as i64;
        if k >= 0 && (k as usize) < x.len() {
            camp[i] = r.conj() * x[k as usize];
        }
    }
    // Two 2000-sample moving averages: a triangle 1/3 s wide.
    let filt = boxcar(&boxcar(&camp, 2000), 2000);
    for (i, r) in reference.iter().enumerate() {
        let k = start + i as i64;
        if k >= 0 && (k as usize) < x.len() {
            x[k as usize] -= 2.0 * (filt[i] * r).re;
        }
    }
}

/// Centred moving average of width `w`, averaging only over the samples
/// that exist near the ends.
fn boxcar(v: &[C32], w: usize) -> Vec<C32> {
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

/// Complex GFSK waveform of the tones at 12 kS/s, unit amplitude.
fn reference_wave(t: &[u8], f0: f32) -> Vec<C32> {
    tones::synth_complex(t, BAUD as f64, 2.0, f0 as f64, RATE as f64)
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
            2.0,
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
