// SPDX-License-Identifier: GPL-3.0-or-later
//! The WSPR-2 decoder: a port of wsprd's `wsprd.c` (Copyright 2001-2018
//! Joe Taylor K1JT; 2014-2018 Steven Franke K9AN; GPL-3.0-or-later),
//! with its defaults: three passes with subtraction, Fano decoding, no
//! OSD (wsprd's OSD is WSJT-X Fortran, GPLv3 only, and off by default).
//!
//! One change: a decoded signal is rebuilt for subtraction from its
//! decoded bits, where wsprd re-packs the printed message (which for a
//! hashed call it doesn't know, `<...>`, gives other bits).

use crate::fano;
use crate::msg::{self, HashTable, Message, PR3};
use num_complex::Complex32 as C32;
use rustfft::FftPlanner;

pub const RATE: f32 = 12_000.0;
/// Samples of a slot the decoder uses (114 s).
pub const NPOINTS_IN: usize = 114 * 12_000;
/// The signal's centre in the audio, Hz above the dial.
pub const CENTRE: f32 = 1500.0;

const NFFT2: usize = 46_080;
const NFFT1: usize = NFFT2 * 32;
const MAXPTS: usize = 65_536;
/// 375 S/s after downsampling.
const DT: f32 = 1.0 / 375.0;
const DF: f32 = 375.0 / 256.0;

#[derive(Clone, Debug)]
pub struct Config {
    /// Search ±this around 1500 Hz (wsprd: 110, -w: 150).
    pub fmax: f32,
    pub passes: usize,
    /// Fano cycles per bit before giving up (wsprd: 10000). On the
    /// corpus 3000 keeps 64 of its 65 decodes for less than half the time.
    pub maxcycles: u32,
    /// Fano metric bias.
    pub bias: f32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            fmax: 110.0,
            passes: 3,
            maxcycles: 3_000,
            bias: 0.45,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Decode {
    pub message: Message,
    /// Audio frequency, Hz above the dial.
    pub freq: f64,
    /// Start relative to 1 s into the slot (wsprd's DT), seconds.
    pub dt: f32,
    /// In 2500 Hz, dB.
    pub snr: f32,
    /// Hz over the transmission.
    pub drift: f32,
    pub sync: f32,
    pub pass: u8,
    pub cycles: u32,
}

#[derive(Clone, Copy, Debug, Default)]
struct Cand {
    freq: f32,
    snr: f32,
    shift: i32,
    drift: f32,
    sync: f32,
}

pub struct Decoder {
    cfg: Config,
    mettab: [[i32; 256]; 2],
    /// Callsigns by hash, kept from slot to slot.
    pub hashes: HashTable,
    /// Slots decoded, and those the deadline cut short.
    pub slots: u64,
    pub cut_short: u64,
}

/// cos and sin per sample of each of the four tones.
type Tables = [(Vec<f32>, Vec<f32>); 4];

/// cos and sin of 2 pi t f dt for the four tones around `fp`, over
/// `n` samples, by the same recurrence as wsprd (in f32).
fn tone_tables(fp: f32, n: usize) -> Tables {
    let twopidt = 2.0 * std::f32::consts::PI * DT;
    let df15 = (DF as f64 * 1.5) as f32;
    let df05 = (DF as f64 * 0.5) as f32;
    let offs = [-df15, -df05, df05, df15];
    std::array::from_fn(|t| {
        let dphi = twopidt * (fp + offs[t]);
        let (cd, sd) = ((dphi as f64).cos() as f32, (dphi as f64).sin() as f32);
        let mut c = vec![0f32; n];
        let mut s = vec![0f32; n];
        c[0] = 1.0;
        for j in 1..n {
            c[j] = c[j - 1] * cd - s[j - 1] * sd;
            s[j] = c[j - 1] * sd + s[j - 1] * cd;
        }
        (c, s)
    })
}

/// The frequency of symbol `i` with a linear drift (0 at the centre).
fn drifted(f0: f32, drift: f32, i: usize) -> f32 {
    (f0 as f64 + (drift as f64 / 2.0) * (i as f64 - 81.0) / 81.0) as f32
}

/// Each symbol's correlation with the four tones: (i, q) per tone.
fn tone_sums(
    id: &[f32],
    qd: &[f32],
    np: usize,
    f0: f32,
    lag: i32,
    drift: f32,
    n: usize,
) -> Vec<[(f32, f32); 4]> {
    let mut out = Vec::with_capacity(162);
    let mut last: Option<(f32, Tables)> = None;
    for i in 0..162 {
        let fp = drifted(f0, drift, i);
        if last.as_ref().is_none_or(|(f, _)| *f != fp) {
            last = Some((fp, tone_tables(fp, n)));
        }
        let tabs = &last.as_ref().unwrap().1;
        let mut r = [(0f32, 0f32); 4];
        for j in 0..256 {
            let k = lag + (i * 256 + j) as i32;
            if k > 0 && (k as usize) < np {
                let (x, y) = (id[k as usize], qd[k as usize]);
                for (t, (c, s)) in tabs.iter().enumerate() {
                    r[t].0 = r[t].0 + x * c[j] + y * s[j];
                    r[t].1 = r[t].1 - x * s[j] + y * c[j];
                }
            }
        }
        out.push(r);
    }
    out
}

/// wsprd's `sync_and_demodulate`, modes 0 (best lag over
/// `lags`) and 1 (best frequency of `f1 + k fstep`, k in `ifs`):
/// returns (sync, shift, freq).
#[allow(clippy::too_many_arguments)]
fn sync_search(
    id: &[f32],
    qd: &[f32],
    np: usize,
    f1: f32,
    ifs: std::ops::RangeInclusive<i32>,
    fstep: f32,
    lags: (i32, i32, i32),
    drift: f32,
) -> (f32, i32, f32) {
    let mut syncmax = -1e30f32;
    let (mut best_shift, mut fbest) = (0, 0.0);
    for ifreq in ifs {
        let f0 = f1 + ifreq as f32 * fstep;
        let mut lag = lags.0;
        while lag <= lags.1 {
            let sums = tone_sums(id, qd, np, f0, lag, drift, 256);
            let mut ss = 0f32;
            let mut totp = 0f32;
            for (i, r) in sums.iter().enumerate() {
                let p: [f32; 4] =
                    std::array::from_fn(|t| (r[t].0 * r[t].0 + r[t].1 * r[t].1).sqrt());
                totp = totp + p[0] + p[1] + p[2] + p[3];
                let cmet = (p[1] + p[3]) - (p[0] + p[2]);
                ss = if PR3[i] == 1 { ss + cmet } else { ss - cmet };
            }
            let ss = ss / totp;
            if ss > syncmax {
                syncmax = ss;
                best_shift = lag;
                fbest = f0;
            }
            lag += lags.2;
        }
    }
    (syncmax, best_shift, fbest)
}

/// Soft symbols (0..255, 128 = nothing) from `symbols` as floats.
fn normalise(fsymb: &[f32; 162], symfac: f32) -> [u8; 162] {
    let mut fsum = 0f32;
    let mut f2sum = 0f32;
    for &v in fsymb {
        fsum += v / 162.0;
        f2sum += v * v / 162.0;
    }
    let fac = (f2sum - fsum * fsum).sqrt();
    std::array::from_fn(|i| {
        let v = (symfac * fsymb[i] / fac).clamp(-128.0, 127.0);
        (v + 128.0) as u8
    })
}

/// wsprd's `noncoherent_sequence_detection`: soft symbols from blocks
/// of `nblock` symbols (1, 2 or 3) taken coherently.
#[allow(clippy::too_many_arguments)]
fn sequence_detect(
    id: &[f32],
    qd: &[f32],
    np: usize,
    f0: f32,
    lag: i32,
    drift: f32,
    symfac: f32,
    nblock: usize,
    bitbybit: bool,
) -> [u8; 162] {
    // per symbol: the tone sums, and each tone's phase step over a symbol
    let mut sums = vec![[(0f32, 0f32); 4]; 162];
    let mut adv = vec![[(0f32, 0f32); 4]; 162];
    let mut last: Option<(f32, Tables)> = None;
    for i in 0..162 {
        let fp = drifted(f0, drift, i);
        if last.as_ref().is_none_or(|(f, _)| *f != fp) {
            last = Some((fp, tone_tables(fp, 257)));
        }
        let tabs = &last.as_ref().unwrap().1;
        for t in 0..4 {
            adv[i][t] = (tabs[t].0[256], tabs[t].1[256]);
        }
        for j in 0..256 {
            let k = lag + (i * 256 + j) as i32;
            if k > 0 && (k as usize) < np {
                let (x, y) = (id[k as usize], qd[k as usize]);
                for (t, (c, s)) in tabs.iter().enumerate() {
                    sums[i][t].0 = sums[i][t].0 + x * c[j] + y * s[j];
                    sums[i][t].1 = sums[i][t].1 - x * s[j] + y * c[j];
                }
            }
        }
    }
    let nseq = 1usize << nblock;
    let mut fsymb = [0f32; 162];
    let mut p = vec![0f32; nseq];
    let mut i = 0;
    while i < 162 {
        for (j, pj) in p.iter_mut().enumerate() {
            let (mut xi, mut xq) = (0f32, 0f32);
            let (mut cm, mut sm) = (1f32, 0f32);
            for ib in 0..nblock {
                let b = (j >> (nblock - 1 - ib)) & 1;
                let t = PR3[i + ib] as usize + 2 * b;
                let (is, qs) = sums[i + ib][t];
                xi = xi + is * cm + qs * sm;
                xq = xq + qs * cm - is * sm;
                let (cf, sf) = adv[i + ib][t];
                let cmp = cf * cm - sf * sm;
                let smp = sf * cm + cf * sm;
                cm = cmp;
                sm = smp;
            }
            *pj = (xi * xi + xq * xq).sqrt();
        }
        for ib in 0..nblock {
            let mask = 1 << (nblock - 1 - ib);
            let (mut xm1, mut xm0) = (0f32, 0f32);
            for (j, &v) in p.iter().enumerate() {
                if j & mask != 0 {
                    xm1 = xm1.max(v);
                } else {
                    xm0 = xm0.max(v);
                }
            }
            fsymb[i + ib] = xm1 - xm0;
            if bitbybit {
                fsymb[i + ib] /= xm1.max(xm0);
            }
        }
        i += nblock;
    }
    normalise(&fsymb, symfac)
}

/// wsprd's `subtract_signal2`: the signal's slowly varying complex
/// amplitude (a 360-sample half-sine low-pass) times its reference.
fn subtract(
    id: &mut [f32],
    qd: &mut [f32],
    np: usize,
    f0: f32,
    shift0: i32,
    drift0: f32,
    channel: &[u8; 162],
) {
    const NSYM: usize = 162;
    const NSPS: usize = 256;
    const NFILT: usize = 360;
    const NC2: usize = 45_000;
    let nsig = NSYM * NSPS;
    let pi = (4.0f64 * 1f64.atan()) as f32;
    let twopidt = (2.0 * pi as f64 * DT as f64) as f32;
    let mut refi = vec![0f32; NC2];
    let mut refq = vec![0f32; NC2];
    let mut phi = 0f32;
    for (i, &cs) in channel.iter().enumerate() {
        let dphi = (twopidt as f64
            * (f0 as f64
                + (drift0 as f64 / 2.0) * (i as f64 - NSYM as f64 / 2.0) / (NSYM as f64 / 2.0)
                + (cs as f64 - 1.5) * DF as f64)) as f32;
        for j in 0..NSPS {
            let ii = NSPS * i + j;
            refi[ii] = (phi as f64).cos() as f32;
            refq[ii] = (phi as f64).sin() as f32;
            phi += dphi;
        }
    }
    let mut w = [0f32; NFILT];
    let mut norm = 0f32;
    for (i, v) in w.iter_mut().enumerate() {
        *v = (pi as f64 * i as f64 / (NFILT - 1) as f64).sin() as f32;
        norm += *v;
    }
    for v in w.iter_mut() {
        *v /= norm;
    }
    let mut partial = [0f32; NFILT];
    for i in 1..NFILT {
        partial[i] = partial[i - 1] + w[i];
    }
    let mut ci = vec![0f32; NC2];
    let mut cq = vec![0f32; NC2];
    for i in 0..nsig {
        let k = shift0 + i as i32;
        if k > 0 && (k as usize) < np {
            let k = k as usize;
            ci[i + NFILT] = id[k] * refi[i] + qd[k] * refq[i];
            cq[i + NFILT] = qd[k] * refi[i] - id[k] * refq[i];
        }
    }
    let mut cfi = vec![0f32; NC2];
    let mut cfq = vec![0f32; NC2];
    for i in NFILT / 2..NC2 - NFILT / 2 {
        let (mut a, mut b) = (0f32, 0f32);
        let base = i - NFILT / 2;
        for j in 0..NFILT {
            a += w[j] * ci[base + j];
            b += w[j] * cq[base + j];
        }
        cfi[i] = a;
        cfq[i] = b;
    }
    for i in 0..nsig {
        let norm = if i < NFILT / 2 {
            partial[NFILT / 2 + i]
        } else if i > nsig - 1 - NFILT / 2 {
            partial[NFILT / 2 + nsig - 1 - i]
        } else {
            1.0
        };
        let k = shift0 + i as i32;
        let j = i + NFILT;
        if k > 0 && (k as usize) < np {
            let k = k as usize;
            id[k] -= (cfi[j] * refi[i] - cfq[j] * refq[i]) / norm;
            qd[k] -= (cfi[j] * refq[i] + cfq[j] * refi[i]) / norm;
        }
    }
}

impl Decoder {
    pub fn new(cfg: Config) -> Self {
        let m =
            |k: usize| (10.0 * (crate::tables::METRIC[k] as f64 - cfg.bias as f64)).round() as i32;
        let mettab = [std::array::from_fn(m), std::array::from_fn(|i| m(255 - i))];
        Decoder {
            cfg,
            mettab,
            hashes: HashTable::default(),
            slots: 0,
            cut_short: 0,
        }
    }

    /// The slot as complex samples at 375 S/s around 1500 Hz (wsprd's
    /// `readwavfile`), zero-padded to 65536.
    fn downsample(audio: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let mut buf = vec![C32::default(); NFFT1];
        for (b, &x) in buf.iter_mut().zip(audio.iter().take(NPOINTS_IN)) {
            b.re = x;
        }
        let mut p = FftPlanner::new();
        p.plan_fft_forward(NFFT1).process(&mut buf);
        let df = 12_000.0 / NFFT1 as f64;
        let i0 = (CENTRE as f64 / df + 0.5) as usize;
        let nh2 = NFFT2 / 2;
        let mut c: Vec<C32> = (0..NFFT2)
            .map(|i| {
                let j = if i > nh2 { i0 + i - NFFT2 } else { i0 + i };
                buf[j]
            })
            .collect();
        p.plan_fft_inverse(NFFT2).process(&mut c);
        let mut id = vec![0f32; MAXPTS];
        let mut qd = vec![0f32; MAXPTS];
        for (k, v) in c.iter().enumerate() {
            id[k] = v.re / 1000.0;
            qd[k] = v.im / 1000.0;
        }
        (id, qd)
    }

    /// Decode a WSPR-2 slot: 12 kS/s USB audio from the even minute (the
    /// first 114 s are used), the band centred 1500 Hz above the dial.
    pub fn decode(&mut self, audio: &[f32]) -> Vec<Decode> {
        self.decode_until(audio, None)
    }

    /// Decode, stopping at `deadline` (candidates go strongest first, so a
    /// short budget loses the weakest).
    pub fn decode_until(
        &mut self,
        audio: &[f32],
        deadline: Option<std::time::Instant>,
    ) -> Vec<Decode> {
        self.slots += 1;
        let late = || deadline.is_some_and(|d| std::time::Instant::now() >= d);
        let (mut id, mut qd) = Self::downsample(audio);
        let np = NFFT2;
        let df = DF / 2.0;
        let symfac = 50.0f32;
        let minsync1 = 0.10f32;
        let minrms = 52.0 * (symfac / 64.0);
        let iifac = 8;
        let delta = 60;
        let nffts = 4 * (np / 512) - 1;
        let win: Vec<f32> = (0..512)
            .map(|i| (0.006147931f64 * i as f64).sin() as f32)
            .collect();
        let mut fft512 = FftPlanner::new();
        let fft512 = fft512.plan_fft_forward(512);

        let mut out: Vec<Decode> = vec![];
        let mut uniques: Vec<(String, f32)> = vec![];
        let mut ndecodes_pass = 0;
        let mut ipass = 0;
        while ipass < self.cfg.passes {
            if ipass == 1 && ndecodes_pass == 0 && self.cfg.passes > 2 {
                ipass = 2;
            }
            let (nblocksize, maxdrift, minsync2) = if ipass < 2 {
                (1, 4, 0.12f32)
            } else {
                (4, 0, 0.10)
            };
            ndecodes_pass = 0;

            // spectrogram: 2-symbol windows, half-symbol steps
            let mut ps = vec![0f32; 512 * nffts];
            let mut buf = vec![C32::default(); 512];
            for i in 0..nffts {
                for (j, b) in buf.iter_mut().enumerate() {
                    let k = i * 128 + j;
                    *b = C32::new(id[k] * win[j], qd[k] * win[j]);
                }
                fft512.process(&mut buf);
                for j in 0..512 {
                    let v = buf[(j + 256) % 512];
                    ps[j * nffts + i] = v.re * v.re + v.im * v.im;
                }
            }
            let mut psavg = [0f32; 512];
            for i in 0..nffts {
                for (j, p) in psavg.iter_mut().enumerate() {
                    *p += ps[j * nffts + i];
                }
            }
            let mut smspec = [0f32; 411];
            for (i, s) in smspec.iter_mut().enumerate() {
                for j in -3i32..=3 {
                    *s += psavg[(256 - 205 + i as i32 + j) as usize];
                }
            }
            let amp: Vec<f32> = ps.iter().map(|p| p.sqrt()).collect();
            let mut sorted = smspec;
            sorted.sort_by(f32::total_cmp);
            let noise = sorted[122];
            let min_snr = 10f64.powf(-0.8) as f32;
            for s in smspec.iter_mut() {
                *s = (*s as f64 / noise as f64 - 1.0) as f32;
                if *s < min_snr {
                    *s = 0.1 * min_snr;
                }
            }
            let mut cands: Vec<Cand> = vec![];
            for j in 1..410 {
                if smspec[j] > smspec[j - 1] && smspec[j] > smspec[j + 1] && cands.len() < 200 {
                    cands.push(Cand {
                        freq: (j as i32 - 205) as f32 * df,
                        snr: (10.0 * (smspec[j] as f64).log10() - 26.3) as f32,
                        ..Default::default()
                    });
                }
            }
            cands.retain(|c| c.freq >= -self.cfg.fmax && c.freq <= self.cfg.fmax);
            // stable, strongest first (wsprd's bubble sort)
            cands.sort_by(|a, b| {
                b.snr
                    .partial_cmp(&a.snr)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            // coarse time, frequency and drift from the spectrogram
            for c in cands.iter_mut() {
                let mut smax = -1e30f32;
                let if0 = (c.freq / df + 256.0) as i32;
                for ifr in if0 - 2..=if0 + 2 {
                    for k0 in -10..22 {
                        for idrift in -maxdrift..=maxdrift {
                            let mut ss = 0f32;
                            let mut pow = 0f32;
                            for (k, &pr) in PR3.iter().enumerate() {
                                let ifd = (ifr as f64
                                    + (k as f64 - 81.0) / 81.0 * idrift as f64 / (2.0 * df as f64))
                                    as i32;
                                let kindex = k0 + 2 * k as i32;
                                if kindex >= 0 && (kindex as usize) < nffts {
                                    let at =
                                        |d: i32| amp[(ifd + d) as usize * nffts + kindex as usize];
                                    let (p0, p1, p2, p3) = (at(-3), at(-1), at(1), at(3));
                                    ss += (2 * pr as i32 - 1) as f32 * ((p1 + p3) - (p0 + p2));
                                    pow = pow + p0 + p1 + p2 + p3;
                                }
                            }
                            let sync1 = ss / pow;
                            if sync1 > smax {
                                smax = sync1;
                                c.shift = 128 * (k0 + 1);
                                c.drift = idrift as f32;
                                c.freq = (ifr - 256) as f32 * df;
                                c.sync = sync1;
                            }
                        }
                    }
                }
            }

            // refine lag and frequency on the sync vector
            for c in cands.iter_mut() {
                let (mut f1, mut drift1, mut shift1) = (c.freq, c.drift, c.shift);
                let lags = (shift1 - 128, shift1 + 128, 64);
                let (s, sh, f) = sync_search(&id, &qd, np, f1, 0..=0, 0.0, lags, drift1);
                let _ = s;
                shift1 = sh;
                f1 = f;
                let (s, _, f) =
                    sync_search(&id, &qd, np, f1, -2..=2, 0.25, (shift1, shift1, 1), drift1);
                let mut sync1 = s;
                f1 = f;
                if ipass < 2 {
                    let one = (shift1, shift1, 1);
                    let (syncp, _, fp) =
                        sync_search(&id, &qd, np, f1, 0..=0, 0.0, one, drift1 + 0.5);
                    f1 = fp;
                    let (syncm, _, fm) =
                        sync_search(&id, &qd, np, f1, 0..=0, 0.0, one, drift1 - 0.5);
                    f1 = fm;
                    if syncp > sync1 {
                        drift1 += 0.5;
                        sync1 = syncp;
                    } else if syncm > sync1 {
                        drift1 -= 0.5;
                        sync1 = syncm;
                    }
                }
                if sync1 > minsync1 {
                    let (s, sh, f) = sync_search(
                        &id,
                        &qd,
                        np,
                        f1,
                        0..=0,
                        0.0,
                        (shift1 - 32, shift1 + 32, 16),
                        drift1,
                    );
                    shift1 = sh;
                    f1 = f;
                    let _ = s;
                    let (s, _, f) =
                        sync_search(&id, &qd, np, f1, -2..=2, 0.05, (shift1, shift1, 1), drift1);
                    sync1 = s;
                    f1 = f;
                    *c = Cand {
                        freq: f1,
                        shift: shift1,
                        drift: drift1,
                        sync: sync1,
                        snr: c.snr,
                    };
                }
            }

            // drop duplicates (keeping the better) and weak syncs
            let mut kept: Vec<Cand> = vec![];
            for c in &cands {
                match kept
                    .iter()
                    .position(|k| (c.freq - k.freq).abs() < 0.05 && (c.shift - k.shift).abs() < 16)
                {
                    Some(k) => {
                        if c.sync > kept[k].sync {
                            kept[k] = *c;
                        }
                    }
                    None if c.sync > minsync2 => kept.push(*c),
                    None => {}
                }
            }

            for c in &kept {
                if late() {
                    self.cut_short += 1;
                    out.sort_by(|a, b| a.freq.total_cmp(&b.freq));
                    return out;
                }
                let (f1, shift1, drift1) = (c.freq, c.shift, c.drift);
                let mut found: Option<(fano::Fano, [u8; 162])> = None;
                let mut ib = 1;
                while ib <= nblocksize && found.is_none() {
                    let (blocksize, bitmetric) = if ib < 4 { (ib, false) } else { (1, true) };
                    let mut idt = 0;
                    while found.is_none() && idt <= 128 / iifac {
                        let mut ii = (idt + 1) / 2;
                        if idt % 2 == 1 {
                            ii = -ii;
                        }
                        ii *= iifac;
                        let mut symbols = sequence_detect(
                            &id,
                            &qd,
                            np,
                            f1,
                            shift1 + ii,
                            drift1,
                            symfac,
                            blocksize,
                            bitmetric,
                        );
                        let sq: f32 = symbols
                            .iter()
                            .map(|&s| {
                                let y = s as f32 - 128.0;
                                y * y
                            })
                            .sum();
                        let rms = (sq / 162.0).sqrt();
                        if rms > minrms {
                            msg::deinterleave(&mut symbols);
                            let f =
                                fano::fano(&symbols, 81, &self.mettab, delta, self.cfg.maxcycles);
                            if f.ok {
                                found = Some((f, symbols));
                            }
                        }
                        idt += 1;
                    }
                    ib += 1;
                }
                let Some((f, _)) = found else { continue };
                ndecodes_pass += 1;
                let mut data = [0u8; 11];
                data[..f.data.len()].copy_from_slice(&f.data);
                let Some(message) = msg::unpack(&data, &mut self.hashes) else {
                    continue;
                };
                let mut d7 = [0u8; 11];
                d7[..7].copy_from_slice(&data[..7]);
                d7[6] &= 0xc0;
                if self.cfg.passes > 1 {
                    subtract(
                        &mut id,
                        &mut qd,
                        np,
                        f1,
                        shift1,
                        drift1,
                        &msg::channel_symbols(&d7),
                    );
                }
                let call = message.text.split(' ').next().unwrap_or("").to_string();
                if uniques
                    .iter()
                    .any(|(c, f)| *c == call && (f1 - f).abs() < 4.0)
                {
                    continue;
                }
                uniques.push((call, f1));
                out.push(Decode {
                    message,
                    freq: CENTRE as f64 + f1 as f64,
                    dt: shift1 as f32 * DT - 1.0,
                    snr: c.snr,
                    drift: drift1,
                    sync: c.sync,
                    pass: ipass as u8 + 1,
                    cycles: f.cycles,
                });
            }
            ipass += 1;
        }
        out.sort_by(|a, b| a.freq.total_cmp(&b.freq));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(n: usize, seed: u64) -> Vec<f32> {
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
                ((-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()) as f32
            })
            .collect()
    }

    /// A WSPR signal at `f` Hz above the dial, starting `dt` s after
    /// 1 s, at `snr` dB in 2500 Hz against unit-variance noise.
    fn add(x: &mut [f32], text: &str, f: f64, dt: f64, snr: f64) {
        let sym = msg::channel_symbols(&msg::pack(text).unwrap());
        let amp = (2.0 * 10f64.powf(snr / 10.0) * 2500.0 / 6000.0).sqrt();
        let start = ((1.0 + dt) * 12_000.0) as usize;
        let mut ph = 0f64;
        for (k, &s) in sym.iter().enumerate() {
            let fk = f + (s as f64 - 1.5) * 12_000.0 / 8192.0;
            for n in 0..8192 {
                let i = start + k * 8192 + n;
                x[i] += (amp * ph.sin()) as f32;
                ph += 2.0 * std::f64::consts::PI * fk / 12_000.0;
            }
        }
    }

    #[test]
    fn decodes_synthetic_signals() {
        let mut x = noise(120 * 12_000, 5);
        add(&mut x, "K1ABC FN42 37", 1480.0, 0.0, -20.0);
        add(&mut x, "G0MBA JO01 23", 1550.3, 0.5, -24.0);
        let mut d = Decoder::new(Config::default());
        let got = d.decode(&x);
        let texts: Vec<&str> = got.iter().map(|d| d.message.text.as_str()).collect();
        assert_eq!(texts, ["K1ABC FN42 37", "G0MBA JO01 23"]);
        let k = &got[0];
        assert!((k.freq - 1480.0).abs() < 0.3, "{k:?}");
        assert!(k.dt.abs() < 0.1, "{k:?}");
        assert!((k.snr + 20.0).abs() < 2.5, "{k:?}");
        assert_eq!(k.message.call.as_deref(), Some("K1ABC"));
        assert_eq!(k.message.grid.as_deref(), Some("FN42"));
        assert_eq!(k.message.dbm, 37);
    }
}
