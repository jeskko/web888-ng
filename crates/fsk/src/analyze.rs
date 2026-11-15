// SPDX-License-Identifier: GPL-3.0-or-later
//! Working out an FSK signal's parameters from the signal (tones, rate,
//! framing, parity, stop bits, polarity, character set; SITOR-B), and the
//! streaming decoder that runs with them. A port of datarx's
//! `wahrx/fsk.py`.

use crate::charset::{self, Ccir476, Ita2, Ita2Auto};
use crate::demod::{self, STD_BAUD, ToneDemod, crossings, estimate_baud, snr_2500, transitions};
use num_complex::Complex32;
use rustfft::FftPlanner;
use web888_cw::dsp;

/// Framing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Start-stop characters.
    Async,
    /// SITOR-B / NAVTEX (CCIR 476 with FEC repetition).
    Sitor,
    /// Synchronous, not decoded (reversals, other codes).
    Sync,
}

/// Parity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parity {
    None,
    Even,
    Odd,
}

/// Everything about one FSK signal.
#[derive(Clone, Debug, PartialEq)]
pub struct FskParams {
    /// Lower and upper tone (Hz, offsets in the analysed input).
    pub f_lo: f64,
    pub f_hi: f64,
    pub baud: f64,
    pub mode: Mode,
    /// Mark (1, the idle state) is the upper tone.
    pub mark_high: bool,
    pub ndata: u32,
    pub parity: Parity,
    pub stop: f64,
    /// ITA2, US-TTY, ASCII, ASCII-8, raw, CCIR476, or what a sync signal was
    /// identified as.
    pub charset: String,
    pub msb_first: bool,
    /// ITA2 unshift on space.
    pub usos: bool,
    pub sitor_off: usize,
    pub sitor_dx: usize,
    /// Framing error rate (async) / invalid code rate (SITOR).
    pub fe_rate: f64,
    /// Text plausibility 0..1.
    pub score: f64,
    /// Signal over noise in 2500 Hz (dB).
    pub snr: f64,
}

impl FskParams {
    pub fn new(f_lo: f64, f_hi: f64, baud: f64) -> Self {
        FskParams {
            f_lo,
            f_hi,
            baud,
            mode: Mode::Async,
            mark_high: true,
            ndata: 5,
            parity: Parity::None,
            stop: 1.5,
            charset: "ITA2".into(),
            msb_first: false,
            usos: true,
            sitor_off: 0,
            sitor_dx: 0,
            fe_rate: 0.0,
            score: 0.0,
            snr: -99.0,
        }
    }

    pub fn shift(&self) -> f64 {
        self.f_hi - self.f_lo
    }

    /// "5N1.5", "SITOR-B" or "sync".
    pub fn framing(&self) -> String {
        match self.mode {
            Mode::Sitor => "SITOR-B".into(),
            Mode::Sync => "sync".into(),
            Mode::Async => {
                let p = match self.parity {
                    Parity::None => 'N',
                    Parity::Even => 'E',
                    Parity::Odd => 'O',
                };
                let s = if self.stop.fract() == 0.0 {
                    format!("{}", self.stop as i64)
                } else {
                    format!("{}", self.stop)
                };
                format!("{}{}{}", self.ndata, p, s)
            }
        }
    }

    fn is_ita2(&self) -> bool {
        self.charset == "ITA2" || self.charset == "US-TTY"
    }
}

// ------------------------------------------------------------------ spectra

fn hann_periodic(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos()) as f32)
        .collect()
}

/// Two-sided spectrogram (frames × bins, power), bins sorted by frequency.
pub fn spectrogram(x: &[Complex32], fs: f64, res: f64) -> (Vec<Vec<f32>>, Vec<f64>) {
    let n = 1usize << ((fs / res).log2().round() as u32);
    let w = hann_periodic(n);
    let fft = FftPlanner::<f32>::new().plan_fft_forward(n);
    let mut out = Vec::new();
    let mut i = 0;
    while i + n <= x.len() {
        let mut b: Vec<Complex32> = (0..n).map(|k| x[i + k] * w[k]).collect();
        fft.process(&mut b);
        // fftshift: bin k (sorted) = (k + n/2) mod n
        out.push((0..n).map(|k| b[(k + n / 2) % n].norm_sqr()).collect());
        i += n / 2;
    }
    let f = (0..n)
        .map(|k| (k as f64 - (n / 2) as f64) * fs / n as f64)
        .collect();
    (out, f)
}

fn peak_f(lvl: &[f32], f: &[f64], i: usize) -> f64 {
    let (a, b, c) = (
        lvl[i.saturating_sub(1)] as f64,
        lvl[i] as f64,
        lvl[(i + 1).min(lvl.len() - 1)] as f64,
    );
    let den = a - 2.0 * b + c;
    f[i] + if den < 0.0 {
        0.5 * (a - c) / den * (f[1] - f[0])
    } else {
        0.0
    }
}

/// Spectral peaks of the spectrogram's mean (local maxima within ±25 Hz,
/// min_db over the floor): (frequency, level dB, power series per frame).
pub fn find_peaks(
    s: &[Vec<f32>],
    f: &[f64],
    min_db: f32,
    floor: Option<&[f32]>,
) -> (Vec<f64>, Vec<f32>, Vec<Vec<f32>>) {
    let nb = f.len();
    let nt = s.len();
    let mut mean = vec![0f64; nb];
    for row in s {
        for (m, v) in mean.iter_mut().zip(row) {
            *m += *v as f64;
        }
    }
    let mean_db: Vec<f32> = mean
        .iter()
        .map(|m| (10.0 * (m / nt as f64 + 1e-20).log10()) as f32)
        .collect();
    let lvl: Vec<f32> = match floor {
        Some(fl) => mean_db.iter().zip(fl).map(|(m, f)| m - f).collect(),
        None => {
            let p30 = dsp::percentile(&mean_db, 30.0);
            mean_db.iter().map(|m| m - p30).collect()
        }
    };
    let df = f[1] - f[0];
    let w = ((25.0 / df).round() as usize).max(1);
    let mut fp = Vec::new();
    let mut lv = Vec::new();
    let mut series = Vec::new();
    for i in w..nb.saturating_sub(w) {
        let pk = (i - w..=i + w).all(|j| lvl[i] >= lvl[j]);
        if pk && lvl[i] >= min_db {
            fp.push(peak_f(&lvl, f, i));
            lv.push(lvl[i]);
            series.push(
                (0..nt)
                    .map(|t| s[t][i - 1] + s[t][i] + s[t][(i + 1).min(nb - 1)])
                    .collect(),
            );
        }
    }
    (fp, lv, series)
}

/// A tone pair: (f_lo, f_hi, level dB, alternation correlation).
pub type Pair = (f64, f64, f32, f64);

/// Tone pairs among peaks: two peaks smin..smax apart whose energies
/// alternate (negative correlation with slow fading removed, or a steady
/// sum while each one swings); most clearly alternating first, each peak
/// used once.
pub fn pair_peaks(
    fp: &[f64],
    lv: &[f32],
    p: &[Vec<f32>],
    smin: f64,
    smax: f64,
    max_diff: f32,
) -> Vec<Pair> {
    let n = fp.len();
    if n < 2 || p.first().is_none_or(|s| s.len() < 8) {
        return Vec::new();
    }
    let mut o: Vec<usize> = (0..n).collect();
    o.sort_by(|&a, &b| fp[a].total_cmp(&fp[b]));
    let nt = p[0].len();
    // log levels with slow fading removed, centred and normalised
    let hn: Vec<Vec<f64>> = o
        .iter()
        .map(|&k| {
            let l: Vec<f64> = p[k].iter().map(|v| (*v as f64 + 1e-20).ln()).collect();
            let mut h: Vec<f64> = (0..nt)
                .map(|t| {
                    let s: f64 = (0..9)
                        .map(|j| {
                            l[(t as isize + j as isize - 4).clamp(0, nt as isize - 1) as usize]
                        })
                        .sum();
                    l[t] - s / 9.0
                })
                .collect();
            let m = h.iter().sum::<f64>() / nt as f64;
            h.iter_mut().for_each(|v| *v -= m);
            let norm = h.iter().map(|v| v * v).sum::<f64>().sqrt() + 1e-20;
            h.iter_mut().for_each(|v| *v /= norm);
            h
        })
        .collect();
    let stat = |k: usize| {
        let s = &p[k];
        let mu = s.iter().map(|&v| v as f64).sum::<f64>() / nt as f64;
        let sd = (s.iter().map(|&v| (v as f64 - mu).powi(2)).sum::<f64>() / nt as f64).sqrt();
        (mu + 1e-20, sd)
    };
    let mut pairs: Vec<Pair> = Vec::new();
    for i in 0..n {
        for j in i + 1..n {
            let (fi, fj) = (fp[o[i]], fp[o[j]]);
            if fj < fi + smin || fj > fi + smax {
                continue;
            }
            let (li, lj) = (lv[o[i]], lv[o[j]]);
            if (li - lj).abs() > max_diff {
                continue;
            }
            let c: f64 = hn[i].iter().zip(&hn[j]).map(|(a, b)| a * b).sum();
            if c >= -0.15 {
                let sm: Vec<f64> = (0..nt)
                    .map(|t| p[o[i]][t] as f64 + p[o[j]][t] as f64)
                    .collect();
                let m = sm.iter().sum::<f64>() / nt as f64;
                let cv = (sm.iter().map(|v| (v - m).powi(2)).sum::<f64>() / nt as f64).sqrt()
                    / (m + 1e-20);
                let ((mi, si), (mj, sj)) = (stat(o[i]), stat(o[j]));
                let cvi = (si / mi).min(sj / mj);
                if !(cv < 0.5 * cvi && cvi > 0.3) {
                    continue;
                }
            }
            pairs.push((fi, fj, li.min(lj), c));
        }
    }
    pairs.sort_by(|a, b| {
        ((a.3 * 10.0).round())
            .total_cmp(&(b.3 * 10.0).round())
            .then(b.2.total_cmp(&a.2))
    });
    let mut out = Vec::new();
    let mut used: Vec<f64> = Vec::new();
    for p in pairs {
        if used
            .iter()
            .any(|&u| (p.0 - u).abs() < 30.0 || (p.1 - u).abs() < 30.0)
        {
            continue;
        }
        used.extend([p.0, p.1]);
        out.push(p);
    }
    out
}

/// Spectral peaks near the given tones (2 Hz bins smoothed over 8 Hz).
pub fn refine_tones(x: &[Complex32], fs: f64, f_lo: f64, f_hi: f64) -> (f64, f64) {
    let mut n = 1usize << ((fs / 2.0).log2().round() as u32);
    if x.len() < n {
        n = 1usize << (x.len().max(2) as f64).log2().floor() as u32;
    }
    // Welch: periodic Hann, 50 % overlap, mean removed per segment
    let w = hann_periodic(n);
    let fft = FftPlanner::<f32>::new().plan_fft_forward(n);
    let mut p = vec![0f64; n];
    let mut i = 0;
    let mut segs = 0;
    while i + n <= x.len() {
        let seg = &x[i..i + n];
        let mean = seg.iter().fold(Complex32::new(0.0, 0.0), |a, v| a + v) / n as f32;
        let mut b: Vec<Complex32> = seg.iter().zip(&w).map(|(v, w)| (v - mean) * *w).collect();
        fft.process(&mut b);
        for k in 0..n {
            p[(k + n / 2) % n] += b[k].norm_sqr() as f64;
        }
        segs += 1;
        i += n / 2;
    }
    if segs == 0 {
        return (f_lo, f_hi);
    }
    let f: Vec<f64> = (0..n)
        .map(|k| (k as f64 - (n / 2) as f64) * fs / n as f64)
        .collect();
    // uniform_filter1d(size 4, reflect): i-2..i+1
    let at = |j: isize| -> f64 {
        let j = if j < 0 {
            -j - 1
        } else if j >= n as isize {
            2 * n as isize - j - 1
        } else {
            j
        };
        p[j as usize]
    };
    let pdb: Vec<f32> = (0..n as isize)
        .map(|i| {
            (10.0 * (((at(i - 2) + at(i - 1) + at(i) + at(i + 1)) / 4.0) + 1e-30).log10()) as f32
        })
        .collect();
    let sep = f_hi - f_lo;
    let win = 10f64.max(25f64.min(0.15 * sep));
    let mut out = [f_lo, f_hi];
    for (o, f0) in out.iter_mut().zip([f_lo, f_hi]) {
        let m: Vec<usize> = (0..n).filter(|&k| (f[k] - f0).abs() <= win).collect();
        if let Some(&k) = m.iter().max_by(|&&a, &&b| pdb[a].total_cmp(&pdb[b])) {
            *o = peak_f(&pdb, &f, k);
        }
    }
    (out[0], out[1])
}

/// scipy's uniform_filter1d (reflect) of complex x.
fn uniform_c(x: &[Complex32], l: usize) -> Vec<Complex32> {
    let n = x.len() as isize;
    let at = |j: isize| -> Complex32 {
        let mut j = j;
        while j < 0 || j >= n {
            j = if j < 0 { -j - 1 } else { 2 * n - j - 1 };
        }
        x[j as usize]
    };
    let lead = (l / 2) as isize;
    let mut out = Vec::with_capacity(x.len());
    let mut s: num_complex::Complex64 = (0..l as isize)
        .map(|k| at(-lead + k))
        .fold(Default::default(), |a, v| {
            a + num_complex::Complex64::new(v.re as f64, v.im as f64)
        });
    for i in 0..n {
        out.push(Complex32::new(
            (s.re / l as f64) as f32,
            (s.im / l as f64) as f32,
        ));
        let old = at(i - lead);
        let new = at(i - lead + l as isize);
        s += num_complex::Complex64::new(
            new.re as f64 - old.re as f64,
            new.im as f64 - old.im as f64,
        );
    }
    out
}

/// Morphological min/max filter of a bool series (window w, centred).
fn all_in_window(b: &[bool], w: usize, want: bool) -> Vec<bool> {
    let n = b.len();
    let half = (w / 2) as isize;
    // count of `want` in the window via prefix sums (edges: nearest)
    let mut cum = vec![0usize; n + 1];
    for i in 0..n {
        cum[i + 1] = cum[i] + usize::from(b[i] == want);
    }
    (0..n as isize)
        .map(|i| {
            let (lo, hi) = (i - half, i - half + w as isize - 1);
            let (a, z) = (lo.max(0) as usize, (hi.min(n as isize - 1)) as usize);
            let inside = cum[z + 1] - cum[a] == z + 1 - a;
            let edge_ok = (lo >= 0 || b[0] == want) && (hi < n as isize || b[n - 1] == want);
            inside && edge_ok
        })
        .collect()
}

/// Tone frequencies from the phase rotation of each tone's bit-filtered
/// output while that tone is clearly on (keying sidebands don't bias it).
pub fn refine_tones_phase(x: &[Complex32], fs: f64, f_lo: f64, f_hi: f64, t: f64) -> (f64, f64) {
    let l = (t.round() as usize).max(2);
    let mut f = [f_lo, f_hi];
    for _ in 0..2 {
        let z: Vec<Vec<Complex32>> = f
            .iter()
            .map(|&fk| {
                let w = -2.0 * std::f64::consts::PI * fk / fs;
                let y: Vec<Complex32> = x
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        let a = w * i as f64;
                        v * Complex32::new(a.cos() as f32, a.sin() as f32)
                    })
                    .collect();
                uniform_c(&y, l)
            })
            .collect();
        let a: Vec<f32> = z[0].iter().map(|v| v.norm()).collect();
        let b: Vec<f32> = z[1].iter().map(|v| v.norm()).collect();
        let hi: Vec<bool> = a.iter().zip(&b).map(|(a, b)| b > a).collect();
        let w = 2 * l + 1;
        let run_hi = all_in_window(&hi, w, true);
        let run_lo = all_in_window(&hi, w, false);
        for k in 0..2 {
            let (on, other, run) = if k == 0 {
                (&a, &b, &run_lo)
            } else {
                (&b, &a, &run_hi)
            };
            let m: Vec<bool> = (0..on.len())
                .map(|i| run[i] && on[i] > 3.0 * other[i])
                .collect();
            let cnt = (1..m.len()).filter(|&i| m[i] && m[i - 1]).count();
            if cnt < 4 * l {
                continue;
            }
            let mut s = num_complex::Complex64::new(0.0, 0.0);
            for i in 1..m.len() {
                if m[i] && m[i - 1] {
                    let v = z[k][i] * z[k][i - 1].conj();
                    s += num_complex::Complex64::new(v.re as f64, v.im as f64);
                }
            }
            f[k] += s.arg() * fs / (2.0 * std::f64::consts::PI);
        }
    }
    (f[0], f[1])
}

// ------------------------------------------------------- async and SITOR

/// One received character.
#[derive(Clone, Debug)]
pub struct Char {
    /// Start-bit edge (stream time, s, or samples without a clock).
    pub t: f64,
    pub code: u32,
    /// Framing error (stop not mark) / invalid code.
    pub fe: bool,
    /// Median amplitude of the keyed tone at the bit ends, and of the other.
    pub on: f32,
    pub off: f32,
    /// Bits since the previous character's start edge.
    pub spacing: f64,
}

fn median(v: &mut [f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f32::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

/// Options of [`async_scan`].
pub struct Scan<'a> {
    pub pos: f64,
    pub end: Option<usize>,
    pub lag: f64,
    pub t_of: Option<&'a dyn Fn(f64) -> f64>,
    pub stop: Option<f64>,
    pub prev: Option<f64>,
}

impl Default for Scan<'_> {
    fn default() -> Self {
        Scan {
            pos: 0.0,
            end: None,
            lag: 0.0,
            t_of: None,
            stop: None,
            prev: None,
        }
    }
}

/// Async start-stop receiver over d (positive = mark): characters of nbits
/// (data and parity) whose frame lies before `end`. Each character is re-timed
/// from all its transitions; with the stop length known, one that follows
/// back-to-back is expected one frame after the previous (a flywheel), so
/// a start edge lost in noise doesn't lose it. Returns (chars, resume
/// position, start of the last character).
pub fn async_scan(
    d: &[f32],
    a: &[f32],
    b: &[f32],
    t: f64,
    nbits: u32,
    o: &Scan,
) -> (Vec<Char>, f64, Option<f64>) {
    let n = d.len();
    let end = o.end.unwrap_or(n) as f64;
    let mut pos = o.pos;
    let mut prev = o.prev;
    if n == 0 {
        return (Vec::new(), pos, prev);
    }
    let cr = crossings(d);
    let falls: Vec<f64> = cr
        .iter()
        .copied()
        .filter(|&c| d[(c.floor() as usize).min(n - 1)] > 0.0)
        .collect();
    let nb = nbits as f64;
    let need = (nb + 1.5) * t + 1.0;
    let frame = o.stop.filter(|&s| s > 0.0).map(|s| (nb + 1.0 + s) * t);
    let di = |p: f64| d[(p.round().max(0.0) as usize).min(n - 1)];
    let mut out = Vec::new();
    let mut k = falls.partition_point(|&f| f < pos);
    loop {
        let mut c: Option<f64> = None;
        if let (Some(fr), Some(pv)) = (frame, prev)
            && pv + fr + need < end
        {
            let pr = pv + fr;
            let j = falls.partition_point(|&f| f < pr - 0.4 * t);
            if j < falls.len() && falls[j] < pr + 0.4 * t {
                c = Some(0.5 * (falls[j] + pr));
            } else if di(pr + 0.5 * t) < 0.0
                && di(pr - 0.5 * t) > 0.0
                && !(j < falls.len() && falls[j] < pr + 0.9 * t)
            {
                c = Some(pr); // edge lost in noise
            }
        }
        let mut c = match c {
            Some(c) => c,
            None => {
                if k >= falls.len() || falls[k] + need >= end {
                    break;
                }
                let c = falls[k];
                if di(c + 0.5 * t) >= 0.0 {
                    k += 1; // the start bit's centre must be space
                    continue;
                }
                c
            }
        };
        // re-time on the transitions inside the frame
        let i0 = cr.partition_point(|&x| x < c + 0.5 * t);
        let i1 = cr.partition_point(|&x| x < c + (nb + 1.5) * t);
        if i1 > i0 {
            let (mut s, mut m) = (0.0, 0usize);
            for &x in &cr[i0..i1] {
                let kk = ((x - c) / t).round();
                let dl = x - c - kk * t;
                if dl.abs() < 0.35 * t && kk >= 1.0 && kk <= nb + 1.0 {
                    s += dl;
                    m += 1;
                }
            }
            c += s / (m + 1) as f64;
        }
        if c + need >= end {
            break;
        }
        let idx: Vec<usize> = (1..=nbits + 1)
            .map(|kb| ((c + (kb as f64 + 0.5) * t).round().max(0.0) as usize).min(n - 1))
            .collect();
        let mut code = 0u32;
        for (i, &ix) in idx.iter().take(nbits as usize).enumerate() {
            if d[ix] > 0.0 {
                code |= 1 << i;
            }
        }
        let fe = d[idx[nbits as usize]] <= 0.0;
        let mut cols = vec![((c + 0.5 * t).round().max(0.0) as usize).min(n - 1)];
        cols.extend(&idx);
        let mut on: Vec<f32> = cols.iter().map(|&i| a[i].max(b[i])).collect();
        let mut off: Vec<f32> = cols.iter().map(|&i| a[i].min(b[i])).collect();
        let sp = prev.map(|p| (c - p) / t).unwrap_or(0.0);
        let tt = match o.t_of {
            Some(f) => f(c - o.lag),
            None => c - o.lag,
        };
        out.push(Char {
            t: tt,
            code,
            fe,
            on: median(&mut on),
            off: median(&mut off),
            spacing: sp,
        });
        prev = Some(c);
        pos = c + (nb + 1.5) * t;
        k = falls.partition_point(|&f| f < pos);
    }
    (out, pos, prev)
}

/// Bit-synchronous sampling with a DPLL on the zero crossings, from t (a
/// bit end on d's time base): (bit values, sample positions, next t).
pub fn sync_bits(
    d: &[f32],
    t_bit: f64,
    mut t: f64,
    end: usize,
    gain: f64,
) -> (Vec<f32>, Vec<f64>, f64) {
    let end = end.min(d.len());
    let cr = crossings(&d[..end]);
    let (mut vals, mut pos) = (Vec::new(), Vec::new());
    let mut k = 0;
    while t + t_bit < end as f64 - 1.0 {
        let e = t - 0.5 * t_bit;
        while k < cr.len() && cr[k] < e - 0.5 * t_bit {
            k += 1;
        }
        if k < cr.len() && cr[k] < e + 0.5 * t_bit {
            t += gain * (cr[k] - e);
        }
        let i = (t.round().max(0.0) as usize).min(end - 1);
        vals.push(d[i]);
        pos.push(t);
        t += t_bit;
    }
    (vals, pos, t)
}

/// Bit offset (0..6) at which 7-bit words have four marks most often.
pub fn sitor_align(bits: &[u8]) -> (usize, f64) {
    let mut best = (0, 0.0);
    for off in 0..7 {
        let n = bits.len().saturating_sub(off) / 7;
        if n < 8 {
            break;
        }
        let sh = sitor_codes(bits, off)
            .iter()
            .filter(|&&c| charset::w4(c))
            .count() as f64
            / n as f64;
        if sh > best.1 {
            best = (off, sh);
        }
    }
    best
}

pub fn sitor_codes(bits: &[u8], off: usize) -> Vec<u32> {
    let n = bits.len().saturating_sub(off) / 7;
    (0..n)
        .map(|w| (0..7).map(|i| u32::from(bits[off + 7 * w + i]) << i).sum())
        .collect()
}

/// The slot parity carrying DX (its slots reappear 5 slots later), and
/// the share of its message slots that do (SITOR-B ~1, other codes ~0).
pub fn sitor_dx_parity(codes: &[u32]) -> (usize, f64) {
    let (mut m, mut n) = ([0usize; 2], [0usize; 2]);
    for i in 0..codes.len().saturating_sub(5) {
        let c = codes[i];
        if charset::w4(c) && c != charset::C476_ALPHA && c != charset::C476_REP {
            n[i & 1] += 1;
            m[i & 1] += usize::from(c == codes[i + 5]);
        }
    }
    let k = usize::from(m[1] > m[0]);
    (k, m[k] as f64 / n[k].max(1) as f64)
}

// ------------------------------------------------------- streaming decoder

/// A decoded character: (time s, text, framed correctly).
pub type Item = (f64, String, bool);

enum Cs {
    Ita2(Ita2Auto),
    Other,
}

/// Decodes a stream with known parameters. Characters whose keyed tone is
/// not clearly above the noise (squelch with hysteresis) are dropped.
pub struct FskDecoder {
    pub fs: f64,
    pub p: FskParams,
    t0: f64,
    t: f64,
    dem: ToneDemod,
    lag: f64,
    d: Vec<f32>,
    a: Vec<f32>,
    b: Vec<f32>,
    base: usize,
    pos: f64,
    prev: Option<f64>,
    nbits: u32,
    cs: Cs,
    c476: Ccir476,
    recent: Vec<bool>,
    levels: Vec<(f32, f32)>,
    noise: Option<f32>,
    sq_open: bool,
    sq_low: u32,
    pub n_chars: usize,
    pub last_good: f64,
    tbit: Option<f64>,
    bits: Vec<u8>,
    bit_t: Vec<f64>,
    bit_lv: Vec<(f32, f32)>,
    slot_done: usize,
}

impl FskDecoder {
    // squelch on keyed-tone over noise amplitude per character: noise alone
    // gives median 1.9, 99th percentile 2.7; ham RTTY at -7 dB 1st
    // percentile 3.0. Opens at OPEN, holds while above HOLD.
    pub const OPEN: f32 = 2.9;
    pub const HOLD: f32 = 2.3;
    /// Per-character threshold of the parameter search.
    pub const QMIN: f32 = 2.6;
    /// The tone filters: a Hann window this many bits long (see
    /// [`ToneDemod::hann`]).
    pub const SPAN: f64 = 1.4;

    pub fn new(fs: f64, p: FskParams, t0: f64) -> Self {
        Self::with_span(fs, p, t0, Self::SPAN)
    }

    /// With the async tone filters a Hann window `span` bits long, or the
    /// 1-bit boxcar for 0.
    pub fn with_span(fs: f64, p: FskParams, t0: f64, span: f64) -> Self {
        let t = fs / p.baud;
        // async: the Hann window (QRM); SITOR keeps the boxcar its bit
        // alignment was found with
        let dem = if p.mode == Mode::Async && span > 0.0 {
            ToneDemod::hann(fs, p.f_lo, p.f_hi, t, span)
        } else {
            ToneDemod::new(fs, p.f_lo, p.f_hi, t)
        };
        let lag = dem.delay();
        let nbits = p.ndata + u32::from(p.parity != Parity::None);
        let cs = if p.is_ita2() {
            Cs::Ita2(Ita2Auto::new(p.usos, p.charset == "US-TTY"))
        } else {
            Cs::Other
        };
        FskDecoder {
            fs,
            p,
            t0,
            t,
            dem,
            lag,
            d: Vec::new(),
            a: Vec::new(),
            b: Vec::new(),
            base: 0,
            pos: 0.0,
            prev: None,
            nbits,
            cs,
            c476: Ccir476::default(),
            recent: Vec::new(),
            levels: Vec::new(),
            noise: None,
            sq_open: false,
            sq_low: 0,
            n_chars: 0,
            last_good: -1e9,
            tbit: None,
            bits: Vec::new(),
            bit_t: Vec::new(),
            bit_lv: Vec::new(),
            slot_done: 0,
        }
    }

    pub fn fe_rate(&self) -> f64 {
        let r = &self.recent[self.recent.len().saturating_sub(60)..];
        if r.is_empty() {
            0.0
        } else {
            r.iter().filter(|&&v| v).count() as f64 / r.len() as f64
        }
    }

    pub fn snr(&self) -> f64 {
        if self.levels.is_empty() {
            return -99.0;
        }
        let lv = &self.levels[self.levels.len().saturating_sub(100)..];
        let mut on: Vec<f32> = lv.iter().map(|v| v.0).collect();
        let mut off: Vec<f32> = lv.iter().map(|v| v.1).collect();
        snr_2500(
            median(&mut on) as f64,
            median(&mut off) as f64,
            self.dem.noise_bw(),
        )
    }

    fn squelch(&mut self, on: f32, off: f32) -> bool {
        let nz = match self.noise {
            None => off,
            Some(n) => 0.9 * n + 0.1 * off,
        };
        self.noise = Some(nz);
        let q = on / nz.max(1e-30);
        if q >= Self::OPEN {
            self.sq_open = true;
            self.sq_low = 0;
        } else if self.sq_open && q < Self::HOLD {
            self.sq_low += 1;
            if self.sq_low >= 2 {
                self.sq_open = false;
            }
        } else if self.sq_open {
            self.sq_low = 0;
        }
        self.sq_open && q >= Self::HOLD
    }

    fn time(&self, i_abs: f64) -> f64 {
        self.t0 + i_abs / self.fs
    }

    pub fn feed(&mut self, x: &[Complex32]) -> Vec<Item> {
        let n0 = self.d.len();
        self.dem.process(x, &mut self.d, &mut self.a, &mut self.b);
        if !self.p.mark_high {
            for v in &mut self.d[n0..] {
                *v = -*v;
            }
        }
        let out = match self.p.mode {
            Mode::Sitor => self.sitor(),
            Mode::Async => self.async_(),
            Mode::Sync => Vec::new(),
        };
        let keep_a = self.pos - self.base as f64 - 4.0 * self.t;
        let keep_b = self.d.len() as f64 - 2.0 * (self.nbits as f64 + 3.0) * self.t;
        let keep = keep_a.min(keep_b).max(0.0) as usize;
        if keep > 0 {
            self.d.drain(..keep);
            self.a.drain(..keep);
            self.b.drain(..keep);
            self.base += keep;
        }
        out
    }

    fn async_(&mut self) -> Vec<Item> {
        let base = self.base as f64;
        let (t0, fs) = (self.t0, self.fs);
        let t_of = move |c: f64| t0 + (c + base) / fs;
        let (chars, pos, prev) = async_scan(
            &self.d,
            &self.a,
            &self.b,
            self.t,
            self.nbits,
            &Scan {
                pos: self.pos - base,
                end: None,
                lag: self.lag,
                t_of: Some(&t_of),
                stop: Some(self.p.stop),
                prev: self.prev.map(|p| p - base),
            },
        );
        self.pos = pos + base;
        self.prev = prev.map(|p| p + base);
        let mut out = Vec::new();
        for ch in chars {
            if !self.squelch(ch.on, ch.off) {
                continue;
            }
            self.recent.push(ch.fe);
            self.n_chars += 1;
            self.last_good = ch.t;
            if !ch.fe {
                self.levels.push((ch.on, ch.off));
            }
            let code = ch.code & ((1 << self.p.ndata) - 1);
            let s: Option<String> = match &mut self.cs {
                Cs::Ita2(cs) => {
                    let c = cs.char(code);
                    self.p.usos = cs.usos;
                    c.map(String::from)
                }
                Cs::Other if self.p.charset.starts_with("ASCII") => {
                    let m = if self.p.charset == "ASCII" {
                        0x7f
                    } else {
                        0xff
                    };
                    charset::ascii_char(code & m, self.p.ndata, self.p.msb_first).map(String::from)
                }
                Cs::Other => Some(format!("<{code:02x}>")),
            };
            if let Some(s) = s {
                out.push((ch.t, s, !ch.fe));
            }
        }
        trim(&mut self.recent, 200);
        trim(&mut self.levels, 200);
        out
    }

    fn sitor(&mut self) -> Vec<Item> {
        let base = self.base as f64;
        let tb = *self.tbit.get_or_insert(base + 0.5 * self.t);
        let (vals, pos, t) = sync_bits(&self.d, self.t, tb - base, self.d.len(), 0.15);
        self.tbit = Some(t + base);
        self.pos = t + base - 2.0 * self.t;
        let n = self.a.len();
        for &q in &pos {
            let i = (q.round().max(0.0) as usize).min(n - 1);
            self.bit_lv
                .push((self.a[i].max(self.b[i]), self.a[i].min(self.b[i])));
            self.bit_t.push(self.time(q + base - self.lag));
        }
        self.bits.extend(vals.iter().map(|&v| u8::from(v > 0.0)));
        let mut out = Vec::new();
        let off = self.p.sitor_off;
        while (self.slot_done + 6) * 7 + off <= self.bits.len() {
            let i = self.slot_done;
            self.slot_done += 1;
            if (i & 1) != self.p.sitor_dx {
                continue;
            }
            let a = off + 7 * i;
            let b = a + 35;
            let word = |s: usize| {
                (0..7)
                    .map(|k| u32::from(self.bits[s + k]) << k)
                    .sum::<u32>()
            };
            let (ca, cb) = (word(a), word(b));
            let mut on: Vec<f32> = self.bit_lv[a..a + 7].iter().map(|v| v.0).collect();
            let mut offv: Vec<f32> = self.bit_lv[a..a + 7].iter().map(|v| v.1).collect();
            let (on, offm) = (median(&mut on), median(&mut offv));
            if !self.squelch(on, offm) {
                continue;
            }
            let code = if charset::w4(ca) {
                Some(ca)
            } else if charset::w4(cb) {
                Some(cb)
            } else {
                None
            };
            self.last_good = self.bit_t[a];
            if matches!(code, Some(c) if [charset::C476_ALPHA, charset::C476_REP, charset::C476_BETA, charset::C476_C32].contains(&c))
            {
                continue; // phasing, idle
            }
            self.recent.push(code.is_none());
            self.n_chars += 1;
            if let Some(c) = code {
                self.levels.push((on, offm));
                if let Some(s) = self.c476.char(c) {
                    out.push((self.bit_t[a], s.to_string(), true));
                }
            }
        }
        let drop = (self.slot_done.saturating_sub(2) / 2) * 14; // whole slot pairs
        if drop > 400 {
            self.bits.drain(..drop);
            self.bit_t.drain(..drop);
            self.bit_lv.drain(..drop);
            self.slot_done -= drop / 7;
        }
        trim(&mut self.recent, 200);
        trim(&mut self.levels, 200);
        out
    }
}

fn trim<T>(v: &mut Vec<T>, n: usize) {
    if v.len() > n {
        v.drain(..v.len() - n);
    }
}

// ------------------------------------------------------------- analysis

/// An analysis: the parameters (None: no FSK found), the text decoded with
/// them, and notes.
#[derive(Clone, Debug, Default)]
pub struct FskResult {
    pub params: Option<FskParams>,
    pub text: String,
    pub chars: Vec<Item>,
    pub baud_fit: f64,
    pub notes: Vec<String>,
}

type Hyps = Vec<((i32, u32), (f64, Vec<Char>))>;

fn async_hypotheses(d: &[f32], a: &[f32], b: &[f32], t: f64, qmin: f32) -> Hyps {
    let mut res = Vec::new();
    let neg: Vec<f32> = d.iter().map(|v| -v).collect();
    for pol in [1, -1] {
        let dd = if pol > 0 { d } else { &neg };
        for nb in 5..=9 {
            let (mut ch, _, _) = async_scan(dd, a, b, t, nb, &Scan::default());
            if !ch.is_empty() {
                let mut offs: Vec<f32> = ch.iter().map(|c| c.off).collect();
                let noise = median(&mut offs);
                ch.retain(|c| c.on >= qmin * noise);
            }
            let fe = if ch.len() >= 8 {
                ch.iter().filter(|c| c.fe).count() as f64 / ch.len() as f64
            } else {
                1.0
            };
            res.push(((pol, nb), (fe, ch)));
        }
    }
    res
}

fn hyp(h: &Hyps, pol: i32, nb: u32) -> &(f64, Vec<Char>) {
    &h.iter()
        .find(|(k, _)| *k == (pol, nb))
        .expect("all hypotheses")
        .1
}

fn stop_bits(chars: &[Char], nb: u32) -> f64 {
    let st: Vec<f64> = chars
        .iter()
        .filter(|c| c.spacing > 0.0 && !c.fe)
        .map(|c| c.spacing - 1.0 - nb as f64)
        .filter(|&s| s > 0.6 && s < 3.0)
        .collect();
    if st.len() < 4 {
        return 1.0;
    }
    let n = st.len() as f64;
    for s in [1.0, 1.5, 2.0] {
        let near = st.iter().filter(|&&v| (v - s).abs() < 0.2).count() as f64;
        let below = st.iter().filter(|&&v| v < s - 0.25).count() as f64;
        if near >= 3f64.max(0.05 * n) && below <= 0.1 * n {
            return s;
        }
    }
    let sf: Vec<f32> = st.iter().map(|&v| v as f32).collect();
    (dsp::percentile(&sf, 5.0) as f64 * 100.0).round() / 100.0
}

/// (charset, msb_first, usos, text, score) best explaining the codes.
fn charset_async(codes: &[u32], nd: u32) -> (String, bool, bool, String, f64) {
    if nd == 5 {
        let mut best: Option<(String, bool, bool, String, f64)> = None;
        for us in [false, true] {
            let usos = charset::ita2_usos(codes, us);
            let mut cs = Ita2::new(usos, us);
            let txt: String = codes.iter().filter_map(|&c| cs.char(c)).collect();
            let mut sc = charset::text_score(&txt);
            if us {
                sc -= 0.01; // ITA2 unless US figures read clearly better
            }
            if best.as_ref().is_none_or(|b| sc > b.4) {
                best = Some((
                    if us { "US-TTY" } else { "ITA2" }.into(),
                    false,
                    usos,
                    txt,
                    sc,
                ));
            }
        }
        return best.expect("two tries");
    }
    if nd == 7 || nd == 8 {
        let mut best: Option<(String, bool, bool, String, f64)> = None;
        for msb in [false, true] {
            let txt: String = codes
                .iter()
                .filter_map(|&c| charset::ascii_char(c, nd, msb))
                .collect();
            let pr = txt.chars().count() as f64 / codes.len().max(1) as f64;
            let sc = 0.5 * pr + 0.5 * charset::text_score(&txt);
            let all7 = codes.iter().all(|c| c >> 7 == 0);
            let any8 = codes.iter().any(|c| c >> 7 != 0);
            let name = if nd == 7 || ((all7 == !msb) && !any8) {
                "ASCII"
            } else {
                "ASCII-8"
            };
            if best.as_ref().is_none_or(|b| sc > b.4) {
                best = Some((name.into(), msb, false, txt, sc));
            }
        }
        return best.expect("two tries");
    }
    (
        "raw".into(),
        false,
        false,
        codes
            .iter()
            .map(|c| format!("{c:02x}"))
            .collect::<Vec<_>>()
            .join(" "),
        0.0,
    )
}

struct Framing {
    pol: i32,
    nb: u32,
    fe: f64,
    stop: f64,
    score: f64,
    ok: bool,
    nd: u32,
    par: Parity,
    cs: (String, bool, bool),
}

fn async_framing(h: &Hyps, pol: i32) -> Framing {
    let fe: Vec<(u32, f64)> = (5..=9).map(|nb| (nb, hyp(h, pol, nb).0)).collect();
    let mfe = fe.iter().map(|v| v.1).fold(f64::INFINITY, f64::min);
    let mut nb = fe
        .iter()
        .filter(|v| v.1 <= 1.5 * mfe + 0.02)
        .map(|v| v.0)
        .max()
        .unwrap_or(5);
    while nb > 5 {
        // trailing always-mark bits are stop bits
        let good: Vec<&Char> = hyp(h, pol, nb).1.iter().filter(|c| !c.fe).collect();
        if !good.is_empty()
            && good
                .iter()
                .filter(|c| (c.code >> (nb - 1)) & 1 == 1)
                .count() as f64
                / good.len() as f64
                > 0.97
        {
            nb -= 1;
        } else {
            break;
        }
    }
    let (fe_nb, chars) = hyp(h, pol, nb);
    let stop = if chars.len() >= 8 {
        stop_bits(chars, nb)
    } else {
        0.0
    };
    let mut r = Framing {
        pol,
        nb,
        fe: *fe_nb,
        stop,
        score: 0.0,
        ok: *fe_nb <= 0.12 && chars.len() >= 8 && stop >= 0.9,
        nd: nb,
        par: Parity::None,
        cs: ("raw".into(), false, false),
    };
    if !r.ok {
        return r;
    }
    let good: Vec<&Char> = chars.iter().filter(|c| !c.fe).collect();
    let (mut nd, mut par) = (nb, Parity::None);
    if nb >= 6 && !good.is_empty() {
        let pe =
            good.iter().filter(|c| c.code.count_ones() % 2 == 0).count() as f64 / good.len() as f64;
        if pe > 0.97 {
            nd = nb - 1;
            par = Parity::Even;
        } else if pe < 0.03 {
            nd = nb - 1;
            par = Parity::Odd;
        }
    }
    let codes: Vec<u32> = good.iter().map(|c| c.code & ((1 << nd) - 1)).collect();
    let (cs, msb, usos, _, score) = charset_async(&codes, nd);
    r.nd = nd;
    r.par = par;
    r.cs = (cs, msb, usos);
    r.score = score;
    r
}

/// Transitions where a tone is clearly above the noise around them.
fn present(tr: &[f64], a: &[f32], b: &[f32], l: f64, fs: f64) -> Vec<f64> {
    if tr.is_empty() {
        return Vec::new();
    }
    let mx: Vec<f32> = a.iter().zip(b).map(|(a, b)| a.max(*b)).collect();
    let mut mn: Vec<f32> = a.iter().zip(b).map(|(a, b)| a.min(*b)).collect();
    let thr = 2.4 * median(&mut mn);
    let w = ((0.05 * fs) as usize).max(3);
    // uniform_filter1d (reflect) of mx
    let env = {
        let n = mx.len() as isize;
        let at = |j: isize| -> f32 {
            let mut j = j;
            while j < 0 || j >= n {
                j = if j < 0 { -j - 1 } else { 2 * n - j - 1 };
            }
            mx[j as usize]
        };
        let lead = (w / 2) as isize;
        let mut s: f64 = (0..w as isize).map(|k| at(-lead + k) as f64).sum();
        let mut e = Vec::with_capacity(mx.len());
        for i in 0..n {
            e.push((s / w as f64) as f32);
            s += at(i - lead + w as isize) as f64 - at(i - lead) as f64;
        }
        e
    };
    let on: Vec<bool> = env.iter().map(|&v| v > thr).collect();
    let on = all_in_window(&on, ((2.0 * l) as usize).max(3) | 1, true);
    let keep: Vec<f64> = tr
        .iter()
        .copied()
        .filter(|&x| on[(x.round().max(0.0) as usize).min(on.len() - 1)])
        .collect();
    if keep.len() >= 12 { keep } else { tr.to_vec() }
}

/// The symbol rate after a short tone filter; longer ones when it's too
/// noisy.
fn baud_of(x: &[Complex32], fs: f64, f_lo: f64, f_hi: f64) -> (f64, f64) {
    let shift = f_hi - f_lo;
    let l0 = 3f64.max((fs / shift).min(fs / 80.0));
    let mut best = (0.0, 0.0);
    for k in [1.0, 2.0, 3.0] {
        let l = l0 * k;
        let (d, a, b) = ToneDemod::new(fs, f_lo, f_hi, l).run(x);
        let tr = present(&transitions(&d, 0.35), &a, &b, l, fs);
        let (bd, fit) = estimate_baud(&tr, fs, 20.0, 700f64.min(1.3 * shift).min(0.9 * fs / l));
        if bd > 0.0 && fit > best.1 + 0.03 {
            best = (bd, fit);
        }
        if best.1 >= 0.85 {
            break;
        }
    }
    best
}

fn sync_snr(d: &[f32], a: &[f32], b: &[f32], t: f64, bw: f64) -> f64 {
    let (_, pos, _) = sync_bits(d, t, 0.5 * t, d.len(), 0.15);
    if pos.is_empty() {
        return -99.0;
    }
    let n = a.len();
    let idx = |q: f64| (q.round().max(0.0) as usize).min(n - 1);
    let mut on: Vec<f32> = pos.iter().map(|&q| a[idx(q)].max(b[idx(q)])).collect();
    let mut off: Vec<f32> = pos.iter().map(|&q| a[idx(q)].min(b[idx(q)])).collect();
    snr_2500(median(&mut on) as f64, median(&mut off) as f64, bw)
}

/// Work out every parameter of the FSK signal in x (complex baseband at
/// fs) and decode it. With tones, the pair is known roughly (refined
/// here); otherwise the strongest alternating pair is used.
pub fn analyze(x: &[Complex32], fs: f64, tones: Option<(f64, f64)>, t0: f64) -> FskResult {
    let (f_lo, f_hi) = match tones {
        Some(t) => t,
        None => {
            let (s, f) = spectrogram(x, fs, 16.0);
            if s.len() < 8 {
                return FskResult {
                    notes: vec!["too short".into()],
                    ..Default::default()
                };
            }
            let (fp, lv, p) = find_peaks(&s, &f, 8.0, None);
            let pr = pair_peaks(&fp, &lv, &p, 60.0, 1100.0, 14.0);
            let Some(p0) = pr.first() else {
                return FskResult {
                    notes: vec!["no FSK tone pair".into()],
                    ..Default::default()
                };
            };
            (p0.0, p0.1)
        }
    };
    let (f_lo, f_hi) = refine_tones(x, fs, f_lo, f_hi);
    if f_hi - f_lo < 20.0 {
        return FskResult {
            notes: vec!["tones too close".into()],
            ..Default::default()
        };
    }
    let (baud, fit) = baud_of(x, fs, f_lo, f_hi);
    if baud == 0.0 {
        return FskResult {
            notes: vec![format!("tones {f_lo:+.0}/{f_hi:+.0} Hz, no symbol rate")],
            ..Default::default()
        };
    }
    let mut res = analyze_at(x, fs, f_lo, f_hi, baud, fit, t0);
    if !STD_BAUD.contains(&baud) {
        // a weak signal's estimate wanders: a standard rate nearby that decodes better wins
        let mut near: Vec<f64> = STD_BAUD.to_vec();
        near.sort_by(|a, b| (a - baud).abs().total_cmp(&(b - baud).abs()));
        for b in near {
            if (b - baud).abs() / baud > 0.08 {
                break;
            }
            let r2 = analyze_at(x, fs, f_lo, f_hi, b, fit, t0);
            if rank(&r2) > rank(&res) {
                res = r2;
            }
        }
    }
    res
}

fn rank(r: &FskResult) -> (i32, i32, i64, i32) {
    let Some(p) = &r.params else {
        return (0, 0, i64::MIN, 0);
    };
    (
        i32::from(is_valid(r, 12)),
        i32::from(p.charset != "raw"),
        ((-p.fe_rate + 0.2 * p.score) * 100.0).round() as i64,
        i32::from(STD_BAUD.contains(&p.baud)),
    )
}

fn analyze_at(
    x: &[Complex32],
    fs: f64,
    f_lo: f64,
    f_hi: f64,
    mut baud: f64,
    mut fit: f64,
    t0: f64,
) -> FskResult {
    let mut notes = Vec::new();
    let mut t = fs / baud;
    let (f_lo, f_hi) = refine_tones_phase(x, fs, f_lo, f_hi, t);
    let mut dem = ToneDemod::new(fs, f_lo, f_hi, t);
    let (mut d, mut a, mut b) = dem.run(x);
    let (b2, fit2) = estimate_baud(&transitions(&d, 0.35), fs, 0.7 * baud, 1.4 * baud);
    if b2 > 0.0 && (b2 - baud).abs() / baud < 0.03 && fit2 >= fit - 0.05 {
        baud = b2;
        fit = fit2;
        t = fs / baud;
        if t.round() as usize != dem.l {
            dem = ToneDemod::new(fs, f_lo, f_hi, t);
            (d, a, b) = dem.run(x);
        }
    }
    let bw = fs / dem.l as f64;
    let mut p = FskParams::new(f_lo, f_hi, baud);
    // reversals: a synchronous system idling
    let (vals, _, _) = sync_bits(&d, t, 0.5 * t, d.len(), 0.15);
    if vals.len() > 50
        && vals
            .windows(2)
            .filter(|w| (w[0] > 0.0) != (w[1] > 0.0))
            .count() as f64
            / (vals.len() - 1) as f64
            > 0.9
    {
        p.mode = Mode::Sync;
        p.charset = "reversals (idle)".into();
        p.snr = sync_snr(&d, &a, &b, t, bw);
        return FskResult {
            params: Some(p),
            baud_fit: fit,
            notes: vec!["bit reversals: a synchronous system idling".into()],
            ..Default::default()
        };
    }
    // async: 5..9 bits, both polarities (some streams frame either way up:
    // 7-bit ASCII sent back to back as 8N1; the text then decides)
    let h = async_hypotheses(&d, &a, &b, t, FskDecoder::QMIN);
    let best_fe = h.iter().map(|v| v.1.0).fold(f64::INFINITY, f64::min);
    let cands: Vec<Framing> = [1, -1]
        .into_iter()
        .filter(|&pol| {
            (5..=9)
                .map(|nb| hyp(&h, pol, nb).0)
                .fold(f64::INFINITY, f64::min)
                <= best_fe + 0.02
        })
        .map(|pol| async_framing(&h, pol))
        .collect();
    let fr = cands
        .into_iter()
        .max_by(|x, y| {
            (x.ok, x.score, -x.fe)
                .partial_cmp(&(y.ok, y.score, -y.fe))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .expect("one polarity has the best rate");
    if fr.ok {
        p.mark_high = fr.pol > 0;
        p.stop = fr.stop;
        p.ndata = fr.nd;
        p.parity = fr.par;
        (p.charset, p.msb_first, p.usos) = fr.cs.clone();
    } else {
        // synchronous 4-of-7 (SITOR-B: CCIR 476, each character repeated)
        let mut sitor: Option<(i32, usize, f64, Vec<u8>)> = None;
        for sp in [1, -1] {
            let dd: Vec<f32> = d.iter().map(|v| v * sp as f32).collect();
            let (vals, _, _) = sync_bits(&dd, t, 0.5 * t, dd.len(), 0.15);
            let bits: Vec<u8> = vals.iter().map(|&v| u8::from(v > 0.0)).collect();
            let (off, share) = sitor_align(&bits);
            if share >= 0.75 && sitor.as_ref().is_none_or(|s| share > s.2) {
                sitor = Some((sp, off, share, bits));
            }
        }
        p.snr = sync_snr(&d, &a, &b, t, bw);
        let Some((sp, off, share, bits)) = sitor else {
            p.mode = Mode::Sync;
            p.charset = "framing unknown".into();
            p.fe_rate = fr.fe;
            notes.push(format!(
                "no async framing (best {} bits, {:.0} % framing errors{}), no 4-of-7 code",
                fr.nb,
                100.0 * fr.fe,
                if fr.stop > 0.0 {
                    format!(", stop {}", fr.stop)
                } else {
                    String::new()
                }
            ));
            return FskResult {
                params: Some(p),
                baud_fit: fit,
                notes,
                ..Default::default()
            };
        };
        let codes = sitor_codes(&bits, off);
        p.mark_high = sp > 0;
        p.ndata = 7;
        p.parity = Parity::None;
        p.stop = 0.0;
        let (dx, reps) = sitor_dx_parity(&codes);
        p.sitor_dx = dx;
        p.sitor_off = off;
        if reps < 0.5 {
            // 4-of-7 words without the FEC repetition (SITOR-A, ARQ-E...): identified only
            p.mode = Mode::Sync;
            p.charset = "4-of-7 code (ITA3/CCIR 476 family, no FEC repeat)".into();
            p.fe_rate = 1.0 - share;
            return FskResult {
                params: Some(p),
                baud_fit: fit,
                notes: vec![format!(
                    "{:.0} % of 7-bit words have 4 marks, {:.0} % repeated 5 slots later",
                    100.0 * share,
                    100.0 * reps
                )],
                ..Default::default()
            };
        }
        p.mode = Mode::Sitor;
        p.charset = "CCIR476".into();
    }
    let mut dec = FskDecoder::new(fs, p.clone(), t0);
    let items = dec.feed(x);
    p.fe_rate = if dec.n_chars > 0 { dec.fe_rate() } else { 1.0 };
    p.snr = dec.snr();
    p.usos = dec.p.usos;
    let text: String = items.iter().map(|i| i.1.as_str()).collect();
    p.score = if p.charset != "raw" {
        charset::text_score(&text)
    } else {
        0.0
    };
    FskResult {
        params: Some(p),
        text,
        chars: items,
        baud_fit: fit,
        notes,
    }
}

/// Decoded well enough to believe the parameters and show the text.
pub fn is_valid(r: &FskResult, min_chars: usize) -> bool {
    let Some(p) = &r.params else {
        return false;
    };
    if p.mode == Mode::Sync {
        return false;
    }
    let n = r.chars.iter().filter(|c| !c.1.trim().is_empty()).count();
    let lim = if p.mode == Mode::Async { 0.12 } else { 0.25 };
    if p.charset == "raw" {
        return n >= min_chars && p.fe_rate <= 0.02;
    }
    if !STD_BAUD.contains(&p.baud) {
        return n >= min_chars && p.fe_rate <= 0.05 && p.score >= 0.7;
    }
    n >= min_chars && p.fe_rate <= lim && p.score >= 0.45
}

/// The same as `demod::snap_baud` (re-exported for callers).
pub fn snap(r: f64) -> f64 {
    demod::snap_baud(r)
}
