// SPDX-License-Identifier: GPL-3.0-or-later
//! The two-tone detector, transitions and the symbol rate.

use num_complex::Complex32;

/// Standard teleprinter and data rates (Bd).
pub const STD_BAUD: [f64; 12] = [
    45.45, 50.0, 56.88, 74.2, 75.0, 96.0, 100.0, 110.0, 150.0, 200.0, 300.0, 600.0,
];

/// Streaming non-coherent two-tone detector. Per input sample: the
/// normalised soft decision (+1 upper tone, −1 lower) and the two tone
/// filters' amplitudes. With [`ToneDemod::new`], output i integrates
/// inputs i−L+1..i (a boxcar, one bit when L = fs/baud): a bit reads
/// cleanly only at its end. [`ToneDemod::hann`] weights a longer span
/// (M = 1.5 L) with a Hann window instead: the same noise bandwidth, but
/// sidelobes 31 dB down and falling 18 dB an octave rather than 13 dB and
/// 6, so a strong neighbour a few hundred Hz away (a contest band) leaks
/// far less. Each tone is normalised by its own tracked on and off
/// levels, so selective fading does not bias the decision.
#[derive(Clone)]
pub struct ToneDemod {
    pub fs: f64,
    pub f: [f64; 2],
    /// The filter's length (samples): L for the boxcar, M for the Hann.
    pub l: usize,
    hann: bool,
    hist: [Vec<Complex32>; 2],
    /// Sliding sums over the last `l` inputs: plain, and weighted by
    /// e^(+j2πn/l), e^(−j2πn/l) (n the absolute index): the Hann window's
    /// three terms.
    sum: [[num_complex::Complex64; 3]; 2],
    /// e^(j2πn/l) for n mod l.
    rot: Vec<num_complex::Complex64>,
    pos: usize,
    ph: [f64; 2],
    alpha: f64,
    ema: Option<[f64; 8]>,
    /// Weigh each tone by its strength now (see [`ToneDemod::set_weighing`]).
    weigh: bool,
    /// Each tone's peak follower, and its per-sample release factor.
    peak: [f64; 2],
    release: f64,
}

impl ToneDemod {
    /// A boxcar of l samples (one bit: fs / baud).
    pub fn new(fs: f64, f_lo: f64, f_hi: f64, l: f64) -> Self {
        Self::make(fs, f_lo, f_hi, (l.round() as usize).max(2), false)
    }

    /// A Hann window `span` × l samples long (l: one bit). 1.5 keeps the
    /// boxcar's noise bandwidth.
    pub fn hann(fs: f64, f_lo: f64, f_hi: f64, l: f64, span: f64) -> Self {
        Self::make(fs, f_lo, f_hi, ((span * l).round() as usize).max(3), true)
    }

    fn make(fs: f64, f_lo: f64, f_hi: f64, l: usize, hann: bool) -> Self {
        let w = 2.0 * std::f64::consts::PI / l as f64;
        ToneDemod {
            fs,
            f: [f_lo, f_hi],
            l,
            hann,
            hist: [
                vec![Complex32::new(0.0, 0.0); l],
                vec![Complex32::new(0.0, 0.0); l],
            ],
            sum: Default::default(),
            rot: (0..l)
                .map(|k| num_complex::Complex64::from_polar(1.0, w * k as f64))
                .collect(),
            pos: 0,
            ph: [0.0; 2],
            alpha: 1.0 / (0.6 * fs),
            ema: None,
            weigh: false,
            peak: [0.0; 2],
            release: (-1.0 / (0.2 * fs)).exp(),
        }
    }

    /// How fast each tone's on and off levels follow (the time constant,
    /// s; 0.6 by default).
    pub fn set_tracking(&mut self, secs: f64) {
        self.alpha = 1.0 / (secs * self.fs);
    }

    /// Selective fading faster than the level tracking: each tone's half
    /// of the decision is weighted by how strong that tone is now (a peak
    /// follower releasing in `secs`, against its tracked on and off
    /// levels), so while one tone is faded into the noise the other
    /// decides alone against its own midpoint, rather than the faded
    /// one's noise counting as much. With both tones up the decision is
    /// unchanged.
    pub fn set_weighing(&mut self, on: bool, secs: f64) {
        self.weigh = on;
        self.release = (-1.0 / (secs * self.fs)).exp();
    }

    /// The filter's delay (samples): a bit's clean reading comes this
    /// long after its centre.
    pub fn delay(&self) -> f64 {
        0.5 * self.l as f64
    }

    /// The tone filters' noise bandwidth (Hz).
    pub fn noise_bw(&self) -> f64 {
        let k = if self.hann { 1.5 } else { 1.0 };
        k * self.fs / self.l as f64
    }

    /// Feeds x; appends the decisions to d and the amplitudes to a, b.
    pub fn process(
        &mut self,
        x: &[Complex32],
        d: &mut Vec<f32>,
        a_out: &mut Vec<f32>,
        b_out: &mut Vec<f32>,
    ) {
        let n0 = a_out.len();
        let two_pi = 2.0 * std::f64::consts::PI;
        let l = self.l;
        let inv_l = 1.0 / l as f64;
        let mut env = [Vec::with_capacity(x.len()), Vec::with_capacity(x.len())];
        for &v in x {
            // the window's first input is n − l + 1; the Hann weights
            // w(m) = ½ − ½cos(2π(m + ½)/l), m = 0..l, in the sliding sums'
            // absolute phase: e^(±j2π(m + ½)/l) = e^(±j2πn'/l) e^(∓j2π(n − l + ½)/l)
            let r = self.rot[self.pos];
            let half = num_complex::Complex64::from_polar(1.0, std::f64::consts::PI * inv_l);
            // e^(−j2π(n − l + ½)/l) = e^(−j2πn/l) e^(−jπ/l); pos = n mod l
            let back = r.conj() * half.conj();
            for k in 0..2 {
                let rot = Complex32::new(self.ph[k].cos() as f32, -self.ph[k].sin() as f32);
                let z = v * rot;
                self.ph[k] += two_pi * self.f[k] / self.fs;
                if self.ph[k] > std::f64::consts::PI {
                    self.ph[k] -= two_pi;
                } else if self.ph[k] < -std::f64::consts::PI {
                    self.ph[k] += two_pi;
                }
                let old = self.hist[k][self.pos];
                self.hist[k][self.pos] = z;
                let dz = num_complex::Complex64::new(
                    z.re as f64 - old.re as f64,
                    z.im as f64 - old.im as f64,
                );
                // old and new share n mod l, so one rotation serves both
                let s = &mut self.sum[k];
                s[0] += dz;
                if self.hann {
                    s[1] += dz * r;
                    s[2] += dz * r.conj();
                    let y = 0.5 * s[0] - 0.25 * (s[1] * back + s[2] * back.conj());
                    env[k].push((y * (2.0 * inv_l)).norm());
                } else {
                    env[k].push((s[0] * inv_l).norm());
                }
            }
            self.pos = (self.pos + 1) % l;
        }
        let al = self.alpha;
        let u = |a: f64, b: f64| {
            let on = if b > a { 1.0 } else { 0.0 };
            [
                a * (1.0 - on),
                b * on,
                1.0 - on,
                on,
                a * on,
                b * (1.0 - on),
                on,
                1.0 - on,
            ]
        };
        let mut y = match self.ema {
            Some(e) => e,
            None => {
                let m = x.len().min((0.3 * self.fs) as usize).max(1);
                let mut s = [0.0; 8];
                for i in 0..m.min(x.len()) {
                    let ui = u(env[0][i], env[1][i]);
                    for k in 0..8 {
                        s[k] += ui[k];
                    }
                }
                s.map(|v| v / m as f64 / (1.0 - al) + 1e-12)
            }
        };
        for i in 0..x.len() {
            let (a, b) = (env[0][i], env[1][i]);
            let ui = u(a, b);
            for k in 0..8 {
                y[k] = al * ui[k] + (1.0 - al) * y[k];
            }
            let mut la = y[0] / y[2].max(1e-6);
            let mut lb = y[1] / y[3].max(1e-6);
            let mut na = y[4] / y[6].max(1e-6);
            let mut nb = y[5] / y[7].max(1e-6);
            // a tone not heard yet (a steady idle tone before any text) has
            // no levels of its own: the other tone's stand in, else noise
            // divided by nothing reads as keying
            if y[2] < 0.02 {
                (la, nb) = (lb, na);
            } else if y[3] < 0.02 {
                (lb, na) = (la, nb);
            }
            let da = (la - na).max(0.15 * la + 1e-12);
            let db = (lb - nb).max(0.15 * lb + 1e-12);
            let (ta, tb) = ((a - na) / da, (b - nb) / db);
            if self.weigh {
                let r = self.release;
                self.peak[0] = a.max(self.peak[0] * r);
                self.peak[1] = b.max(self.peak[1] * r);
                let wa = ((self.peak[0] - na) / da).clamp(0.1, 1.0);
                let wb = ((self.peak[1] - nb) / db).clamp(0.1, 1.0);
                d.push((2.0 * (wb * (tb - 0.5) - wa * (ta - 0.5)) / (wa + wb)) as f32);
            } else {
                d.push((tb - ta) as f32);
            }
            a_out.push(a as f32);
            b_out.push(b as f32);
        }
        if !x.is_empty() {
            self.ema = Some(y);
        }
        debug_assert_eq!(a_out.len() - n0, x.len());
    }

    /// The whole of x at once: (d, a, b).
    pub fn run(&mut self, x: &[Complex32]) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
        let (mut d, mut a, mut b) = (Vec::new(), Vec::new(), Vec::new());
        self.process(x, &mut d, &mut a, &mut b);
        (d, a, b)
    }
}

/// SNR (dB in 2500 Hz) from the median on-tone and off-tone amplitudes at
/// bit ends; the off tone carries Rayleigh noise (E|n|² = 1.443 median²)
/// in the bit filter's bandwidth bw.
pub fn snr_2500(on_med: f64, off_med: f64, bw: f64) -> f64 {
    let pn = 1.443 * off_med * off_med + 1e-30;
    let ps = (on_med * on_med - pn).max(1e-3 * pn);
    10.0 * (ps / pn * bw / 2500.0).log10()
}

/// Zero crossings of s (fractional sample positions).
pub fn crossings(s: &[f32]) -> Vec<f64> {
    let mut out = Vec::new();
    for i in 0..s.len().saturating_sub(1) {
        if (s[i] > 0.0) != (s[i + 1] > 0.0) {
            let (a, b) = (s[i] as f64, s[i + 1] as f64);
            out.push(i as f64 + a / (a - b + 1e-30));
        }
    }
    out
}

/// Zero crossings of d between excursions beyond ±thr (noise wiggles near
/// zero ignored).
pub fn transitions(d: &[f32], thr: f32) -> Vec<f64> {
    let strong: Vec<usize> = (0..d.len()).filter(|&i| d[i].abs() > thr).collect();
    if strong.len() < 2 {
        return Vec::new();
    }
    let cr = crossings(d);
    if cr.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for w in strong.windows(2) {
        if (d[w[0]] > 0.0) != (d[w[1]] > 0.0) {
            // the last crossing before strong[i+1], if after strong[i]
            let k = cr.partition_point(|&c| c < w[1] as f64);
            if k >= 1 && cr[k - 1] > w[0] as f64 {
                out.push(cr[k - 1]);
            }
        }
    }
    out
}

pub fn snap_baud(r: f64) -> f64 {
    let best = STD_BAUD
        .iter()
        .copied()
        .min_by(|a, b| ((a - r).abs() / a).total_cmp(&((b - r).abs() / b)))
        .unwrap_or(r);
    if (best - r).abs() / best < 0.015 {
        best
    } else {
        (r * 100.0).round() / 100.0
    }
}

/// Baud from transition times (samples at fs): the most common shortest
/// interval with nothing systematic below it, refined by least squares.
/// Returns (baud, fit: share of intervals that are whole or half bits).
pub fn estimate_baud(tr: &[f64], fs: f64, rmin: f64, rmax: f64) -> (f64, f64) {
    if tr.len() < 3 {
        return (0.0, 0.0);
    }
    let mut iv: Vec<f64> = tr
        .windows(2)
        .map(|w| (w[1] - w[0]) / fs)
        .filter(|&v| v > 0.4 / rmax && v < 24.0 / rmin)
        .collect();
    iv.sort_by(f64::total_cmp);
    let iv2: Vec<f64> = tr.windows(3).map(|w| (w[2] - w[0]) / fs).collect();
    if iv.len() < 12 {
        return (0.0, 0.0);
    }
    let count = |lo: f64, hi: f64| {
        iv.partition_point(|&v| v < hi) as f64 - iv.partition_point(|&v| v < lo) as f64
    };
    let (mut best, mut bt) = (f64::NEG_INFINITY, 0.0);
    let mut lt = (1.0 / rmax).ln();
    while lt < (1.0 / rmin).ln() {
        let t = lt.exp();
        let sc = count(0.85 * t, 1.15 * t) - 2.0 * count(0.3 * t, 0.8 * t);
        if sc > best {
            best = sc;
            bt = t;
        }
        lt += 0.004;
    }
    if best < 6.0 {
        return (0.0, 0.0);
    }
    let mut c: Vec<f64> = iv
        .iter()
        .copied()
        .filter(|&v| v > 0.8 * bt && v < 1.2 * bt)
        .collect();
    c.sort_by(f64::total_cmp);
    let mut t = if c.is_empty() {
        bt
    } else if c.len() % 2 == 1 {
        c[c.len() / 2]
    } else {
        0.5 * (c[c.len() / 2 - 1] + c[c.len() / 2])
    };
    for _ in 0..3 {
        let (mut num, mut den, mut n) = (0.0, 0.0, 0);
        for &v in &iv {
            let k = (v / t).round();
            if (1.0..=12.0).contains(&k) && (v / t - k).abs() < 0.12 {
                num += k * v;
                den += k * k;
                n += 1;
            }
        }
        if n < 6 {
            return (0.0, 0.0);
        }
        t = num / den;
    }
    for _ in 0..2 {
        let (mut num, mut den, mut n) = (0.0, 0.0, 0);
        for &v in &iv2 {
            let k = (v / t).round();
            if (2.0..=24.0).contains(&k) && (v / t - k).abs() < 0.12 {
                num += k * v;
                den += k * k;
                n += 1;
            }
        }
        if n >= 6 {
            t = num / den;
        }
    }
    let fit = iv
        .iter()
        .filter(|&&v| {
            let h = (2.0 * v / t).round() / 2.0;
            h >= 1.0 && (v / t - h).abs() < 0.15
        })
        .count() as f64
        / iv.len() as f64;
    (snap_baud(1.0 / t), fit)
}
