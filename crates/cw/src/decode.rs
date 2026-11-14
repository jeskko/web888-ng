// SPDX-License-Identifier: GPL-3.0-or-later
//! CW decoding of one channel: complex baseband at [`ENV_FS`] with the
//! carrier near 0 Hz. A port of datarx's `wahrx/cw.py` (decode path).
//!
//! The carrier is found to 0.5 Hz; for each speed hypothesis the channel
//! is low-passed to ~1.5/dot, its power smoothed over 0.6 dot and cut into
//! marks and spaces halfway (dB) between the tracked noise floor and mark
//! level, never near the noise envelope's tail (no keyed signal: nothing
//! decoded). The hypothesis whose marks and spaces best fit 1:3 and 1:3:7
//! wins; the dot length then adapts element by element (hand keying) and
//! the character/word gap boundary is learned from the sender's spaces.
//! The input is first cut at silent gaps so each over of a QSO gets its
//! own frequency and speed.

use crate::dsp::{self, Rng};
use crate::morse;
use num_complex::Complex32;
use rustfft::FftPlanner;
use std::cell::RefCell;
use std::collections::HashMap;

/// The envelope (channel) rate: 2 ms.
pub const ENV_FS: f64 = 500.0;

/// Speed hypotheses (WPM) tried when the speed is unknown.
const SPEEDS: [f64; 6] = [45.0, 32.0, 24.0, 18.0, 13.0, 9.0];

/// A decoded stretch of CW.
#[derive(Clone, Debug, Default)]
pub struct CwResult {
    pub text: String,
    pub wpm: f64,
    /// Carrier offset (Hz) in the input.
    pub freq: f64,
    /// Mark power over noise in 2500 Hz (dB).
    pub snr: f64,
    /// (time s, character), spaces included.
    pub chars: Vec<(f64, String)>,
    pub marks: usize,
    /// Timing fit (mean squared log error; ~0.02 clean, > 0.1 poor).
    pub cost: f64,
}

impl CwResult {
    fn empty(freq: f64) -> Self {
        CwResult {
            freq,
            snr: -99.0,
            ..Default::default()
        }
    }

    /// Worth showing: Morse-like timing, usable SNR, a human speed.
    pub fn plausible(&self) -> bool {
        self.marks >= 3 && self.cost < 0.1 && self.snr > -10.0 && (6.0..=50.0).contains(&self.wpm)
    }
}

thread_local! {
    static FIR: RefCell<HashMap<i64, Vec<f32>>> = RefCell::new(HashMap::new());
    static NOISE_CAL: RefCell<HashMap<(i64, usize), (f32, f32)>> = RefCell::new(HashMap::new());
    static FFT: RefCell<FftPlanner<f32>> = RefCell::new(FftPlanner::new());
}

fn fir(bw: f64) -> Vec<f32> {
    let key = bw.round() as i64;
    FIR.with_borrow_mut(|m| {
        m.entry(key)
            .or_insert_with(|| {
                dsp::firwin(
                    ((ENV_FS / key as f64 * 4.0) as usize) | 1,
                    key as f64,
                    ENV_FS,
                )
            })
            .clone()
    })
}

/// |y|² after a ±bw low-pass.
fn lowpass_power(y: &[Complex32], bw: f64) -> Vec<f32> {
    dsp::convolve_same(y, &fir(bw))
        .iter()
        .map(|v| v.norm_sqr())
        .collect()
}

fn mix(x: &[Complex32], f: f64) -> Vec<Complex32> {
    if f == 0.0 {
        return x.to_vec();
    }
    let w = -2.0 * std::f64::consts::PI * f / ENV_FS;
    x.iter()
        .enumerate()
        .map(|(i, v)| {
            let a = w * i as f64;
            v * Complex32::new(a.cos() as f32, a.sin() as f32)
        })
        .collect()
}

fn db(p: &[f32]) -> Vec<f32> {
    p.iter().map(|v| 10.0 * (v + 1e-20).log10()).collect()
}

/// Carrier frequency near f0 (±span) from the averaged spectrum (0.5 Hz
/// bins, parabolic peak), and the peak's level over the median of the
/// searched span (dB).
pub fn fine_freq(x: &[Complex32], f0: f64, span: f64) -> (f64, f64) {
    let nf = 1usize << ((ENV_FS / 0.5).log2().ceil() as u32);
    let win: Vec<f32> = (0..nf)
        .map(|i| {
            (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (nf - 1) as f64).cos()) as f32
        })
        .collect();
    let fft = FFT.with_borrow_mut(|p| p.plan_fft_forward(nf));
    let mut p = vec![0.0f64; nf];
    let mut buf = vec![Complex32::new(0.0, 0.0); nf];
    let hop = nf / 2;
    let mut i = 0;
    loop {
        for (k, b) in buf.iter_mut().enumerate() {
            *b = x.get(i + k).copied().unwrap_or_default() * win[k];
        }
        fft.process(&mut buf);
        for (pk, b) in p.iter_mut().zip(&buf) {
            *pk += b.norm_sqr() as f64;
        }
        i += hop;
        if i + nf > x.len() {
            break;
        }
    }
    let freq = |k: usize| if k < nf / 2 { k as f64 } else { k as f64 - nf as f64 } * ENV_FS / nf as f64;
    let idx: Vec<usize> = (0..nf).filter(|&k| (freq(k) - f0).abs() <= span).collect();
    let Some(&i) = idx.iter().max_by(|&&a, &&b| p[a].total_cmp(&p[b])) else {
        return (f0, 0.0);
    };
    let (a, b, c) = (
        (p[(i + nf - 1) % nf] + 1e-30).ln(),
        (p[i] + 1e-30).ln(),
        (p[(i + 1) % nf] + 1e-30).ln(),
    );
    let den = a - 2.0 * b + c;
    let d = if den < 0.0 { 0.5 * (a - c) / den } else { 0.0 };
    let fr = freq(i) + d * ENV_FS / nf as f64;
    let span_p: Vec<f32> = idx.iter().map(|&k| p[k] as f32).collect();
    let med = (dsp::median(&span_p) as f64).max(1e-30);
    (fr, 10.0 * (p[i] / med).log10())
}

/// dB offsets of the smoothed noise envelope's mean and 99.5th percentile
/// above its 20th percentile, for this bandwidth and smoothing (simulated
/// once). The noise envelope is far from Gaussian with few independent
/// samples per window, so a threshold must clear its tail.
fn noise_cal(bw: f64, ns: usize) -> (f32, f32) {
    let key = (bw.round() as i64, ns);
    if let Some(v) = NOISE_CAL.with_borrow(|m| m.get(&key).copied()) {
        return v;
    }
    let mut rng = Rng::new(12345);
    let z: Vec<Complex32> = (0..40000)
        .map(|_| Complex32::new(rng.normal() as f32, rng.normal() as f32))
        .collect();
    let p = lowpass_power(&z, key.0 as f64);
    let p = dsp::smooth(&p, ns);
    let p = &p[500..p.len() - 500];
    let mean = p.iter().map(|&v| v as f64).sum::<f64>() / p.len() as f64;
    let d = db(p);
    let p20 = dsp::percentile(&d, 20.0);
    let v = (
        10.0 * mean.log10() as f32 - p20,
        dsp::percentile(&d, 99.5) - p20,
    );
    NOISE_CAL.with_borrow_mut(|m| m.insert(key, v));
    v
}

/// Noise floor (sliding 20th percentile over 4 s) and mark level (90th
/// percentile over ~12 units, held through short deep fades), in dB.
fn levels(ps: &[f32], unit_n: f64) -> (Vec<f32>, Vec<f32>) {
    let d_all = db(ps);
    let dec = ((unit_n / 4.0) as usize).max(1);
    let d: Vec<f32> = d_all.iter().step_by(dec).copied().collect();
    let nlo = ((4.0 * ENV_FS / dec as f64) as usize).max(3);
    let nhi = ((12.0 * unit_n / dec as f64) as usize).max(3);
    let lo = dsp::percentile_filter(&d, 20.0, nlo);
    let mut hi = dsp::percentile_filter(&d, 90.0, nhi);
    let hold = ((2.0 * ENV_FS / dec as f64) as usize).max(3);
    let h97 = dsp::percentile_filter(&d, 97.0, hold);
    for (h, v) in hi.iter_mut().zip(h97) {
        *h = h.max(v - 12.0);
    }
    (
        dsp::interp_decimated(&lo, dec, ps.len()),
        dsp::interp_decimated(&hi, dec, ps.len()),
    )
}

/// (on, start, length) runs.
type Run = (bool, usize, usize);

fn runs(b: &[bool]) -> Vec<Run> {
    let mut out = Vec::new();
    let mut s = 0;
    for i in 1..=b.len() {
        if i == b.len() || b[i] != b[s] {
            out.push((b[s], s, i - s));
            s = i;
        }
    }
    out
}

/// Marks from the envelope, with the noise floor + mean offset and the
/// mark level (dB) used.
fn binarize(p: &[f32], unit_n: f64, bw: f64, edge: usize) -> Vec<bool> {
    let ns = ((0.6 * unit_n) as usize).max(1);
    let ps = dsp::smooth(p, ns);
    // the filters' start-up at either end is not noise: the levels see the
    // nearest settled sample there instead
    let mut pl = ps.clone();
    let n = pl.len();
    if edge > 0 && 2 * edge < n {
        let (a, b) = (pl[edge], pl[n - 1 - edge]);
        pl[..edge].fill(a);
        pl[n - edge..].fill(b);
    }
    let (lo, hi) = levels(&pl, unit_n);
    let (off_mean, off_tail) = noise_cal(bw, ns);
    let d = db(&ps);
    let mut on = vec![false; n];
    let mut state = false;
    for i in 0..n {
        let thr = (0.5 * (lo[i] + hi[i]))
            .max(hi[i] - 8.0)
            .max(lo[i] + off_mean + 3.0);
        let mut active = hi[i] >= lo[i] + off_tail + 1.0;
        if edge > 0 && (i < edge || i + edge >= n) {
            active = false;
        }
        if active && d[i] > thr + 1.0 {
            state = true;
        } else if !active || d[i] < thr - 1.0 {
            state = false;
        }
        on[i] = state;
    }
    on
}

fn geomspace(a: f64, b: f64, n: usize) -> Vec<f64> {
    let (la, lb) = (a.ln(), b.ln());
    (0..n)
        .map(|i| (la + (lb - la) * i as f64 / (n - 1) as f64).exp())
        .collect()
}

/// Dot length (samples) that best explains marks as 1 or 3 units (and
/// spaces as 1, 3 or 7).
pub fn estimate_unit(marks: &[f64], spaces: &[f64]) -> Option<f64> {
    if marks.len() < 3 {
        return None;
    }
    let mf: Vec<f32> = marks.iter().map(|&v| v as f32).collect();
    let lo = (dsp::percentile(&mf, 5.0) as f64 * 0.5).max(2.0);
    let hi = dsp::percentile(&mf, 95.0) as f64 * 1.2;
    let med = dsp::median(&mf) as f64;
    let sp: Vec<f64> = spaces.iter().copied().filter(|&s| s < 12.0 * med).collect();
    let (l1, l3, l7) = (0.0f64, 3f64.ln(), 7f64.ln());
    let mut best = (f64::INFINITY, lo);
    for c in geomspace(lo, hi.max(lo * 1.01), 160) {
        let mut cost = 0.0;
        let mut n1 = 0usize;
        for &m in marks {
            let lr = (m / c).ln();
            let e = lr.abs().min((lr - l3).abs());
            cost += e.min(0.7).powi(2);
            if lr.abs() < (lr - l3).abs() {
                n1 += 1;
            }
        }
        cost /= marks.len() as f64;
        if !sp.is_empty() {
            let mut cs = 0.0;
            for &s in &sp {
                let ls = (s / c).ln();
                let es = (ls - l1).abs().min((ls - l3).abs()).min((ls - l7).abs());
                cs += es.min(0.7).powi(2);
            }
            cost += 0.5 * cs / sp.len() as f64;
        }
        let f1 = n1 as f64 / marks.len() as f64;
        if !(0.05..=0.95).contains(&f1) {
            cost += 0.05;
        }
        if cost < best.0 {
            best = (cost, c);
        }
    }
    Some(best.1)
}

/// Character/word gap boundary (units) from the sender's spaces: an Otsu
/// split of the log lengths of spaces of 2-20 units; 5 if unclear.
pub fn word_gap(spaces: &[f64], unit: f64) -> f64 {
    let r: Vec<f64> = spaces
        .iter()
        .map(|s| s / unit)
        .filter(|&v| (2.0..20.0).contains(&v))
        .map(f64::ln)
        .collect();
    if r.len() < 6 {
        return 5.0;
    }
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let mut best = -1.0;
    let mut thr = 5.0;
    for i in 0..35 {
        let t = (3.6 + (7.0 - 3.6) * i as f64 / 34.0).ln();
        let a: Vec<f64> = r.iter().copied().filter(|&v| v < t).collect();
        let b: Vec<f64> = r.iter().copied().filter(|&v| v >= t).collect();
        if a.len() < 2 || b.len() < 2 {
            continue;
        }
        let v = a.len() as f64 * b.len() as f64 * (mean(&a) - mean(&b)).powi(2);
        if v > best {
            best = v;
            thr = t.exp();
        }
    }
    if best < 0.0 {
        return 5.0;
    }
    let a: Vec<f64> = r.iter().copied().filter(|&v| v < thr.ln()).collect();
    let b: Vec<f64> = r.iter().copied().filter(|&v| v >= thr.ln()).collect();
    if mean(&b) - mean(&a) < 1.5f64.ln() {
        return 5.0;
    }
    (0.5 * (mean(&a) + mean(&b))).exp()
}

/// Runs to characters, the unit adapting to the keying. Spaces of
/// 2..wgap units end a character, longer ones a word.
fn runs_to_text(runs: &[Run], unit: f64, t0: f64, wgap: f64) -> (Vec<(f64, String)>, f64) {
    const ADAPT: f64 = 0.15;
    let mut out = Vec::new();
    let mut code = String::new();
    let mut u = unit;
    let mut t_char: Option<usize> = None;
    for &(on, start, n) in runs {
        let nf = n as f64;
        let r = nf / u;
        if on {
            t_char.get_or_insert(start);
            let obs = if r < 2.0 {
                code.push('.');
                nf
            } else {
                code.push('-');
                nf / 3.0
            };
            if 0.4 < obs / u && obs / u < 2.0 {
                u = (1.0 - ADAPT) * u + ADAPT * obs;
            }
        } else {
            if r < 2.0 {
                if 0.5 < r && r < 1.6 {
                    u = (1.0 - ADAPT / 2.0) * u + ADAPT / 2.0 * nf;
                }
                continue;
            }
            if !code.is_empty() {
                out.push((
                    t0 + t_char.unwrap_or(start) as f64 / ENV_FS,
                    morse::decode(&code).to_string(),
                ));
                code.clear();
                t_char = None;
            }
            if r >= wgap {
                out.push((t0 + (start + n) as f64 / ENV_FS, " ".to_string()));
            }
        }
    }
    if !code.is_empty() {
        out.push((
            t0 + t_char.unwrap_or(0) as f64 / ENV_FS,
            morse::decode(&code).to_string(),
        ));
    }
    (out, u)
}

/// Glitches (spaces shorter than min_space, marks shorter than min_mark)
/// merged into the previous run.
fn clean_runs(rr: &[Run], min_space: f64, min_mark: f64) -> Vec<Run> {
    let mut clean: Vec<Run> = Vec::new();
    for &(mut st, start, n) in rr {
        let mut short = (n as f64) < if st { min_mark } else { min_space };
        if short && st && clean.is_empty() {
            st = false;
            short = false;
        }
        if let Some(last) = clean.last_mut()
            && (short || last.0 == st)
        {
            last.2 += n;
            continue;
        }
        clean.push((st, start, n));
    }
    let mut out: Vec<Run> = Vec::new();
    for r in clean {
        match out.last_mut() {
            Some(l) if l.0 == r.0 => l.2 += r.2,
            _ => out.push(r),
        }
    }
    out
}

fn fit_cost(marks: &[f64], spaces: &[f64], u: f64) -> f64 {
    if marks.is_empty() {
        return f64::INFINITY;
    }
    let l3 = 3f64.ln();
    let mut cost = marks
        .iter()
        .map(|m| {
            let r = (m / u).ln();
            r.abs().min((r - l3).abs()).min(0.7).powi(2)
        })
        .sum::<f64>()
        / marks.len() as f64;
    let sp: Vec<f64> = spaces.iter().map(|s| s / u).filter(|&s| s < 12.0).collect();
    if !sp.is_empty() {
        let es = sp
            .iter()
            .map(|s| {
                [1.0f64, 3.0, 7.0]
                    .iter()
                    .map(|k| (s / k).ln().abs())
                    .fold(f64::INFINITY, f64::min)
                    .min(0.7)
                    .powi(2)
            })
            .sum::<f64>()
            / sp.len() as f64;
        cost += 0.5 * es;
    }
    cost
}

struct Hyp {
    cost: f64,
    p: Vec<f32>,
    on: Vec<bool>,
    clean: Vec<Run>,
    nmarks: usize,
    u2: f64,
    bw: f64,
}

fn marks_spaces(clean: &[Run]) -> (Vec<f64>, Vec<f64>) {
    let marks = clean.iter().filter(|r| r.0).map(|r| r.2 as f64).collect();
    let spaces = clean.iter().filter(|r| !r.0).map(|r| r.2 as f64).collect();
    (marks, spaces)
}

/// One speed hypothesis on y (carrier at 0 Hz).
fn decode_at(y: &[Complex32], unit_n: f64, known: bool) -> Hyp {
    let bw = (1.5 * ENV_FS / unit_n).clamp(15.0, 120.0);
    let p = lowpass_power(y, bw);
    let edge = (2.0 * ENV_FS / bw + 0.5 * unit_n) as usize + 2;
    let on = binarize(&p, unit_n, bw, edge);
    let clean = clean_runs(&runs(&on), 0.3 * unit_n, 0.5 * unit_n);
    let (marks, spaces) = marks_spaces(&clean);
    let mut u2 = estimate_unit(&marks, &spaces).unwrap_or(unit_n);
    if known && !(0.7 < u2 / unit_n && u2 / unit_n < 1.4) {
        u2 = unit_n; // a known speed only drifts, never jumps x2/x3
    }
    let cost = fit_cost(&marks, &spaces, u2);
    Hyp {
        cost,
        p,
        on,
        clean,
        nmarks: marks.len(),
        u2,
        bw,
    }
}

fn search(y: &[Complex32], speeds: &[f64], known: bool) -> Option<Hyp> {
    let mut best: Option<Hyp> = None;
    for &w in speeds {
        let unit_n = synth_unit(w) * ENV_FS;
        let mut h = decode_at(y, unit_n, known);
        if h.nmarks < 3 {
            continue;
        }
        if !known && !(0.6 < h.u2 / unit_n && h.u2 / unit_n < 1.7) {
            // inconsistent: refine once at the estimate
            h = decode_at(y, h.u2, false);
            if h.nmarks < 3 {
                continue;
            }
        }
        if best.as_ref().is_none_or(|b| h.cost < b.cost) {
            best = Some(h);
        }
    }
    best
}

fn synth_unit(wpm: f64) -> f64 {
    1.2 / wpm
}

/// One transmission: fine frequency, then the speed (a known speed first;
/// the hypotheses if that fits poorly), marks and spaces, text.
fn decode_one(x: &[Complex32], f0: Option<f64>, wpm: Option<f64>, t0: f64, span: f64) -> CwResult {
    let (f, prom) = fine_freq(
        x,
        f0.unwrap_or(0.0),
        if f0.is_none() { ENV_FS / 2.0 } else { span },
    );
    if prom < 8.0 {
        return CwResult::empty(f); // no carrier here
    }
    let y = mix(x, f);
    let mut best = wpm.and_then(|w| search(&y, &[w], true));
    // unknown or changed speed (another operator): the hypotheses
    if best.as_ref().is_none_or(|b| b.cost > 0.06)
        && let Some(alt) = search(&y, &SPEEDS, false)
    {
        let take = match &best {
            None => true,
            Some(b) => {
                alt.cost < (0.04f64).min(0.6 * b.cost) && alt.nmarks as f64 >= 0.7 * b.nmarks as f64
            }
        };
        if take {
            best = Some(alt);
        }
    }
    let Some(h) = best else {
        return CwResult::empty(f);
    };
    let (_, spaces) = marks_spaces(&h.clean);
    let (chars, _) = runs_to_text(&h.clean, h.u2, t0, word_gap(&spaces, h.u2));
    let mut text = String::new();
    for (_, c) in &chars {
        if c == " " && (text.is_empty() || text.ends_with(' ')) {
            continue;
        }
        text.push_str(c);
    }
    let text = text.trim_end().to_string();
    // SNR: mark level over noise floor in the measurement bandwidth -> 2500 Hz
    let pm: Vec<f32> =
        h.p.iter()
            .zip(&h.on)
            .filter(|(_, o)| **o)
            .map(|(p, _)| *p)
            .collect();
    let pn: Vec<f32> =
        h.p.iter()
            .zip(&h.on)
            .filter(|(_, o)| !**o)
            .map(|(p, _)| *p)
            .collect();
    let snr = if !pm.is_empty() && !pn.is_empty() {
        let sm = dsp::median(&pm) as f64;
        let nm = dsp::median(&pn) as f64;
        let enbw = h.bw * 2.0;
        10.0 * ((sm - nm).max(1e-20) / (nm / enbw * 2500.0)).log10()
    } else {
        -99.0
    };
    CwResult {
        text,
        wpm: 1.2 * ENV_FS / h.u2,
        freq: f,
        snr,
        chars,
        marks: h.nmarks,
        cost: h.cost,
    }
}

/// Cut x at the middle of silent gaps (no keyed activity within ±span of
/// f0 for ≥ gap s); pieces shorter than min_len s join the previous one.
pub fn transmissions(
    x: &[Complex32],
    f0: Option<f64>,
    span: f64,
    gap: f64,
    min_len: f64,
) -> Vec<(usize, usize)> {
    let y = mix(x, f0.unwrap_or(0.0));
    let bw = span.min(0.45 * ENV_FS);
    let ns = (0.03 * ENV_FS) as usize;
    let p = dsp::smooth(&lowpass_power(&y, bw), ns);
    let d = db(&p);
    let lo = dsp::percentile(&d, 20.0);
    let (_, off_tail) = noise_cal(bw, ns);
    let mut act: Vec<bool> = d.iter().map(|&v| v > lo + off_tail + 2.0).collect();
    let e = ((0.1 * ENV_FS) as usize).min(act.len());
    for a in act[..e].iter_mut() {
        *a = false;
    }
    let len = act.len();
    for a in act[len - e..].iter_mut() {
        *a = false;
    }
    let mut cuts = vec![0usize];
    for (st, start, n) in runs(&act) {
        if !st && n as f64 >= gap * ENV_FS && start > 0 && start + n < len {
            let c = start + n / 2;
            if (c - cuts[cuts.len() - 1]) as f64 / ENV_FS >= min_len {
                cuts.push(c);
            }
        }
    }
    if cuts.len() > 1 && (x.len() - cuts[cuts.len() - 1]) as f64 / ENV_FS < min_len {
        cuts.pop();
    }
    cuts.push(x.len());
    cuts.windows(2).map(|w| (w[0], w[1])).collect()
}

/// Options for [`decode`].
#[derive(Clone, Debug)]
pub struct DecodeOpts {
    /// Carrier near this offset (default: the strongest peak).
    pub f0: Option<f64>,
    /// ±span around f0 (Hz).
    pub span: f64,
    /// A speed to try first.
    pub wpm: Option<f64>,
    /// Time of x[0] (s).
    pub t0: f64,
    /// Split into transmissions first.
    pub split: bool,
    /// Leave out transmissions that don't look like Morse.
    pub strict: bool,
}

impl Default for DecodeOpts {
    fn default() -> Self {
        DecodeOpts {
            f0: None,
            span: 150.0,
            wpm: None,
            t0: 0.0,
            split: true,
            strict: false,
        }
    }
}

/// Decode CW in complex baseband x at [`ENV_FS`]. Each transmission gets
/// its own frequency and speed; the returned wpm/freq/snr are the last
/// one's.
pub fn decode(x: &[Complex32], o: &DecodeOpts) -> CwResult {
    if x.len() < (ENV_FS as usize) {
        return CwResult::empty(o.f0.unwrap_or(0.0));
    }
    let segs = if o.split {
        transmissions(x, o.f0, o.span, 1.5, 2.0)
    } else {
        vec![(0, x.len())]
    };
    let mut wpm = o.wpm;
    let mut res: Vec<CwResult> = Vec::new();
    for (a, b) in segs {
        let r = decode_one(&x[a..b], o.f0, wpm, o.t0 + a as f64 / ENV_FS, o.span);
        if r.marks > 0 && (!o.strict || r.plausible()) {
            if r.cost < 0.06 {
                wpm = Some(r.wpm); // the next over is likely the same speed range
            }
            res.push(r);
        }
    }
    let Some(last) = res.last() else {
        return CwResult::empty(o.f0.unwrap_or(0.0));
    };
    let mut chars: Vec<(f64, String)> = Vec::new();
    for r in &res {
        if chars.last().is_some_and(|c| c.1 != " ") {
            chars.push((r.chars.first().map(|c| c.0).unwrap_or(0.0), " ".into()));
        }
        chars.extend(r.chars.iter().cloned());
    }
    let text = res
        .iter()
        .filter(|r| !r.text.is_empty())
        .map(|r| r.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let marks: usize = res.iter().map(|r| r.marks).sum();
    let cost = res.iter().map(|r| r.cost * r.marks as f64).sum::<f64>() / marks.max(1) as f64;
    CwResult {
        text,
        wpm: last.wpm,
        freq: last.freq,
        snr: last.snr,
        chars,
        marks,
        cost,
    }
}
