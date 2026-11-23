// SPDX-License-Identifier: GPL-3.0-or-later
//! Measurements of one signal ("what is this?"): what an analyst reads
//! off a signal before naming it. From 4.096 s of 12 kS/s audio and the
//! audio range the listener picked:
//!
//! * where it is: its extent over the floor, its centre, the width of
//!   90 % of its power and its SNR in its extent;
//! * its spectrum: tones (peaks standing out of their surroundings), their
//!   common spacing, how much of the power is in one narrow line (a
//!   carrier), and whether that line has sidebands on both sides (AM);
//! * its envelope: on/off keying (how much of the time it is on, the
//!   bursts and the shortest of them), how much the level varies while on
//!   (constant-envelope FSK against speech or multi-tone modems), and how
//!   much of that variation is at syllable rates (speech);
//! * its symbols: the symbol rate from lines in the envelope's spectrum
//!   (PSK and other filtered modems) or in the spectrum of its frequency
//!   transitions (FSK); for PSK the order from the lowest power of the
//!   signal that shows a line (2: BPSK, 4: QPSK, 8: 8-PSK);
//! * its structure: a repetition period (a frame, a probe, a pulse
//!   train) from the autocorrelation of the signal or of its envelope;
//! * its drift (Hz/s).
//!
//! Every figure says only what was measured; whether it is good enough to
//! use (the SNR, the line strengths) is the reader's to judge, and the
//! text built from it (`ui/src/sigdesc.ts`) says so.
//!
//! Frequencies are audio Hz (LSB audio is the RF upside down: the caller
//! maps them).

use crate::scan::AUDIO_RATE;
use num_complex::Complex32;
use rustfft::FftPlanner;

/// Audio samples measured (4.096 s).
pub const NA: usize = 2 * crate::N;

/// A spectral line or tone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    /// Hz (audio for tones; a rate for symbol rates; s for periods).
    pub at: f64,
    /// How far it stands out (dB; for periods the correlation 0..1).
    pub db: f64,
}

/// A sweep ([`Measure::chirp`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chirp {
    pub slope: f64,
    pub n: u32,
    pub r2: f64,
    /// The bursts' median length (s): a pulsed radar's pulse (its sweep
    /// slope x len wide), an FMCW sweep's time in the window.
    pub len: f64,
    /// Sweeps a second from flybacks inside the window (a sweep narrower
    /// than the window: always on, a sawtooth).
    pub rate: Option<f64>,
}

/// A tone grid ([`Measure::grid`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    /// Spacing (Hz), tones used, how periodic the histogram is (0..1).
    pub spacing: f64,
    pub n: u32,
    pub strength: f64,
    /// Frame length that showed it best (s), and how often the strongest
    /// tone changes (per s).
    pub frame: f64,
    pub changes: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Measure {
    /// The signal's extent (3 dB over the floor, smoothed over 25 Hz),
    /// its power-weighted centre (audio Hz), and the width of 90 % of its
    /// power (Hz; a carrier with weak sidebands: narrow).
    pub lo: f64,
    pub hi: f64,
    pub centre: f64,
    pub bw90: f64,
    /// SNR in the occupied band (dB), from the floor of the passband.
    pub snr: f64,
    /// It fills the passband (no floor beside it: the SNR, the keying
    /// and the extent are not to be trusted; widen the passband).
    pub fills: bool,
    /// Fraction of the time on (0..1); on periods; their mean and
    /// shortest typical (25th percentile) length (s).
    pub duty: f64,
    pub bursts: u32,
    pub on_mean: f64,
    pub on_short: f64,
    /// How deep the off times are under the on times (dB, medians): keying
    /// drops to the noise (20 dB and more), fading and flicker near the
    /// threshold don't.
    pub depth: f64,
    /// Envelope while on: standard deviation over mean of the amplitude
    /// within 100-ms blocks (the median block: fading left out).
    pub env_cv: f64,
    /// Share of the envelope's variation at 1-10 Hz (speech: high).
    pub syllabic: f64,
    /// Peak to average power while on (dB, the 99th percentile of the
    /// samples' power, noise taken out): ~0 for FSK and carriers, 2-4 for
    /// filtered single-carrier PSK, ~6.6 for OFDM and other many-carrier
    /// or noise-like signals.
    pub papr: f64,
    /// The signal's power kurtosis E|s|^4 / E|s|^2^2 with the noise's
    /// share taken out exactly (its power known): 1 for a constant
    /// envelope (FSK, a carrier), ~1.3-1.6 for single-carrier PSK, 2 for
    /// OFDM and other Gaussian-like signals.
    pub kurt: f64,
    /// Tones, strongest first (audio Hz, dB over the floor); their common
    /// spacing when three or more fit one grid.
    pub tones: Vec<Line>,
    pub spacing: Option<f64>,
    /// Two-tone FSK: the shift (Hz) from its frequency track (the two
    /// levels of the instantaneous frequency, smoothed over half a bit,
    /// where the signal is strong); spectral peaks are not the tones when
    /// the shift is a few times the rate or less.
    pub shift: Option<f64>,
    /// Tones on a grid seen frame by frame (as on a waterfall: each short
    /// frame's strongest frequency, their histogram's period): MFSK's
    /// hopping tones, parallel-tone modems' carriers.
    pub grid: Option<Grid>,
    /// Share of the power in the strongest narrow line (±3 Hz).
    pub line: f64,
    /// The strongest line: where (audio Hz), and the share of the power
    /// 20 Hz or more below and above it (AM: both, alike; AM heard in
    /// SSB: one).
    pub line_at: f64,
    pub side_lo: f64,
    pub side_hi: f64,
    /// How alike the spectra below and above that line are (correlation of
    /// their log power, mirrored about it): AM's sidebands are each
    /// other's image (~1), other signals beside a carrier aren't.
    pub mirror: f64,
    /// Every narrow line (±3 Hz) with 5 % of the power or more (audio Hz,
    /// share): carriers side by side (AM stations 5 kHz apart) show here.
    pub carriers: Vec<Line>,
    /// Symbol rate (Bd, from the envelope), and from frequency transitions.
    pub baud_env: Option<Line>,
    pub baud_freq: Option<Line>,
    /// PSK order from the power that shows a line (2, 4, 8), with its line
    /// (`at`: the carrier offset from `centre`, Hz; `db`: its share of the
    /// power, 0..1).
    pub psk: Option<(u32, Line)>,
    /// Repetition period (s) of the signal (a sequence repeating) and of
    /// its envelope (pulses, frames); `db` is the correlation (0..1).
    pub period: Option<Line>,
    pub env_period: Option<Line>,
    /// The strongest narrow peak of |R| at 2-40 ms (s, correlation): a
    /// guard interval's copy, so OFDM's useful symbol length.
    pub symbol: Option<Line>,
    /// Drift (audio Hz/s) when it is steady enough to say.
    pub drift: Option<f64>,
    /// Sweeps (radars, sounders): the frequency's slope along each burst
    /// (Hz/s, + upwards), how many bursts glide alike, and how straight
    /// (r^2) the median one is. A sweep of width B repeating R times a
    /// second crosses the window at B R Hz/s.
    pub chirp: Option<Chirp>,
}

fn fft(x: &mut [Complex32], inverse: bool) {
    let mut p = FftPlanner::new();
    if inverse {
        p.plan_fft_inverse(x.len()).process(x)
    } else {
        p.plan_fft_forward(x.len()).process(x)
    }
}

fn percentile(v: &[f64], q: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    let k = ((s.len() - 1) as f64 * q).round() as usize;
    *s.select_nth_unstable_by(k, |a, b| a.total_cmp(b)).1
}

fn db(v: f64) -> f64 {
    10.0 * (v + 1e-30).log10()
}

/// Moving average over 2h+1 (shorter at the ends).
fn smooth(v: &[f64], h: usize) -> Vec<f64> {
    let mut c = vec![0.0; v.len() + 1];
    for (i, x) in v.iter().enumerate() {
        c[i + 1] = c[i] + x;
    }
    (0..v.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(h), (i + h + 1).min(v.len()));
            (c[b] - c[a]) / (b - a) as f64
        })
        .collect()
}

/// The strongest line of a power spectrum `p` (bin width `df`) between
/// `f0` and `f1` Hz: its frequency (parabolic) and how far its three
/// bins stand over the median of ±`ctx` Hz around it (dB).
fn find_line(p: &[f64], df: f64, f0: f64, f1: f64, ctx: f64) -> Option<Line> {
    let k0 = ((f0 / df).ceil() as usize).max(2);
    let k1 = ((f1 / df).floor() as usize).min(p.len() - 3);
    if k1 <= k0 {
        return None;
    }
    let w = (ctx / df).round().max(8.0) as usize;
    let mut best: Option<(f64, usize)> = None;
    // candidates: the 12 largest three-bin sums, judged by their context
    let mut cand: Vec<(f64, usize)> = (k0..=k1).map(|k| (p[k - 1] + p[k] + p[k + 1], k)).collect();
    cand.sort_by(|a, b| b.0.total_cmp(&a.0));
    for &(s, k) in cand.iter().take(12) {
        if p[k] < p[k - 1] || p[k] < p[k + 1] {
            continue;
        }
        let (a, b) = (k.saturating_sub(w).max(1), (k + w + 1).min(p.len()));
        let ctxv: Vec<f64> = (a..b)
            .filter(|&j| j + 3 < k || j > k + 3)
            .map(|j| p[j])
            .collect();
        let med = percentile(&ctxv, 0.5) * 3.0;
        let r = db(s / (med + 1e-30));
        if best.is_none_or(|(rb, _)| r > rb) {
            best = Some((r, k));
        }
    }
    let (r, k) = best?;
    let (a, b, c) = (p[k - 1].sqrt(), p[k].sqrt(), p[k + 1].sqrt());
    let d = 0.5 * (a - c) / (a - 2.0 * b + c).min(-1e-30);
    Some(Line {
        at: (k as f64 + d) * df,
        db: r,
    })
}

/// Power spectrum of a real feature (mean removed), padded to `n`
/// points (bin: rate / n); its first half.
fn feature_spectrum(v: &[f64], n: usize) -> Vec<f64> {
    let m = v.iter().sum::<f64>() / v.len() as f64;
    let mut z: Vec<Complex32> = v
        .iter()
        .map(|&x| Complex32::new((x - m) as f32, 0.0))
        .collect();
    z.resize(n, Complex32::new(0.0, 0.0));
    fft(&mut z, false);
    z[..n / 2].iter().map(|c| c.norm_sqr() as f64).collect()
}

/// The first peak of a normalised autocorrelation `r` (lag 0 = 1) after
/// it has fallen below half, at lags from `l0`; with its value, if it
/// stands well over the median beyond it.
fn first_period(r: &[f64], l0: usize, rate: f64) -> Option<Line> {
    let n = r.len();
    let mut i = l0.max(1);
    while i < n && r[i] > 0.5 {
        i += 1;
    }
    if i + 4 >= n {
        return None;
    }
    let med = percentile(&r[i..], 0.5);
    let (k, &v) = r[i..]
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))?;
    let k = k + i;
    // the fundamental: the first local maximum nearly as high
    let k = (i..k)
        .find(|&j| r[j] >= 0.8 * v && r[j] >= r[j - 1] && r[j] >= r[j + 1])
        .unwrap_or(k);
    let v = r[k];
    // a peak, not the tail of the fall from lag 0
    let dip = r[i..=k].iter().copied().fold(f64::MAX, f64::min);
    if k < i + 2 || v - dip < 0.1 || v < 0.15 || v < 4.0 * med {
        return None;
    }
    let (a, b, c) = (r[k - 1], r[k], r.get(k + 1).copied().unwrap_or(r[k]));
    let d = 0.5 * (a - c) / (a - 2.0 * b + c).min(-1e-12);
    Some(Line {
        at: (k as f64 + d.clamp(-0.5, 0.5)) / rate,
        db: v,
    })
}

/// Measure the signal in audio `sel` (Hz) of the last [`NA`] samples of
/// `audio` (12 kS/s); `band` is the receiver's audio passband (its floor
/// is the noise). None if nothing stands over the floor there.
pub fn measure(audio: &[f32], sel: (f64, f64), band: (f64, f64)) -> Option<Measure> {
    if audio.len() < NA {
        return None;
    }
    let x: Vec<Complex32> = audio[audio.len() - NA..]
        .iter()
        .map(|&v| Complex32::new(v, 0.0))
        .collect();
    core(&x, AUDIO_RATE, NA / 2, sel, band)
}

/// Samples of complex baseband at `rate` that [`measure_iq`] reads.
pub fn iq_len(rate: f64) -> usize {
    (4.096 * rate).round() as usize
}

/// The same on the last 4.096 s of complex baseband `iq` at `rate`
/// (S/s: the receiver's IQ, as wide as ±rate/2); `sel` and `band` in Hz
/// from its centre (below it negative), and so are the frequencies
/// measured.
pub fn measure_iq(
    iq: &[Complex32],
    rate: f64,
    sel: (f64, f64),
    band: (f64, f64),
) -> Option<Measure> {
    let n = iq_len(rate);
    if iq.len() < n {
        return None;
    }
    // up by half the rate (every other sample negated): bin k of its
    // spectrum is k df - rate / 2, all of them positive frequencies here
    let h = rate / 2.0;
    let x: Vec<Complex32> = iq[iq.len() - n..]
        .iter()
        .enumerate()
        .map(|(i, &v)| if i % 2 == 1 { -v } else { v })
        .collect();
    let mut m = core(
        &x,
        rate,
        n,
        (sel.0 + h, sel.1 + h),
        (band.0 + h, band.1 + h),
    )?;
    m.lo -= h;
    m.hi -= h;
    m.centre -= h;
    m.line_at -= h;
    for t in m.tones.iter_mut().chain(m.carriers.iter_mut()) {
        t.at -= h;
    }
    Some(m)
}

/// `x`: complex samples at `rate` whose first `kmax` FFT bins are the
/// frequencies looked at (k rate / len Hz); `sel` and `band` in those Hz.
fn core(
    x: &[Complex32],
    rate: f64,
    kmax: usize,
    sel: (f64, f64),
    band: (f64, f64),
) -> Option<Measure> {
    let n = x.len();
    let dfa = rate / n as f64;
    // the baseband the signal is cut out to: half the rate for real audio
    // (its positive half), the whole of it for IQ (as wide as it is)
    let (fs, nb) = if kmax == n {
        (rate, n)
    } else {
        (rate / 2.0, n / 2)
    };
    // one FFT of the window: everything else is cut from it
    let mut xa = x.to_vec();
    fft(&mut xa, false);
    let pa: Vec<f64> = xa[..kmax].iter().map(|c| c.norm_sqr() as f64).collect();
    let bin = |hz: f64| ((hz / dfa).round().max(1.0) as usize).min(kmax - 1);
    let (b0, b1) = (bin(band.0.max(50.0)), bin(band.1));
    let (s0, s1) = (bin(sel.0.max(band.0)), bin(sel.1.min(band.1)));
    if b1 <= b0 + 100 || s1 <= s0 + 8 {
        return None;
    }
    // the floor: from a spectrogram (85-ms frames, smoothed over five
    // bins, ~60 Hz), the 10th percentile over time and frequency, the
    // outer 5 % of the passband left out (the filter's skirts): the gaps
    // in speech and between bursts count, and a signal filling up to ~85 %
    // of the passband for all the time leaves it alone
    let floor = floor_of(x, b0, b1, rate);
    let sw = smooth(&pa, (12.5 / dfa) as usize);
    // the spectrum smoothed over ~3 Hz; a signal: some bins clearly over
    // the floor
    let sp = smooth(&pa, 6);
    let strongest = |a: usize, b: usize| {
        (a..=b)
            .map(|k| (k, sp[k]))
            .max_by(|x, y| x.1.total_cmp(&y.1))
    };
    let (seed, peak) = strongest(s0, s1)?;
    if peak < 4.0 * floor {
        return None;
    }
    // the middle third first: a range picked around a signal (a wide
    // look's) has its neighbours at the sides
    let third = (s1 - s0) / 3;
    let seed = match strongest(s0 + third, s1 - third) {
        Some((k, p)) if p >= 4.0 * floor && third > 0 => k,
        _ => seed,
    };
    // its extent: around the strongest part, the bins over the floor by
    // 3 dB smoothed over 25 Hz, gaps up to 1 kHz joined (a voice, a
    // modem, a keyed carrier's sidebands, 850 Hz FSK, AM's sidebands: one
    // signal; the range picked keeps its neighbours out)
    let gap = (1000.0 / dfa) as usize;
    let over = |k: usize| sw[k] > 2.0 * floor;
    let (mut lo, mut hi) = (seed, seed);
    let mut k = seed;
    while k > s0 && k + gap > lo {
        k -= 1;
        if over(k) {
            lo = k;
        }
    }
    k = seed;
    while k < s1 && k < hi + gap {
        k += 1;
        if over(k) {
            hi = k;
        }
    }
    let hi = hi.max(lo + 4);
    let ex: Vec<f64> = (lo..=hi).map(|k| (sp[k] - floor).max(0.0)).collect();
    let tot: f64 = ex.iter().sum::<f64>().max(1e-30);
    let centre = ex
        .iter()
        .enumerate()
        .map(|(i, v)| (lo + i) as f64 * v)
        .sum::<f64>()
        / tot
        * dfa;
    // 90 % of the power: 5 % cut on each side
    let edge = |it: &mut dyn Iterator<Item = (usize, &f64)>| {
        let mut acc = 0.0;
        for (i, v) in it {
            acc += v;
            if acc >= 0.05 * tot {
                return i;
            }
        }
        0
    };
    let (a90, b90) = (
        edge(&mut ex.iter().enumerate()),
        edge(&mut ex.iter().enumerate().rev()),
    );
    let bw90 = (b90.saturating_sub(a90) + 1) as f64 * dfa;
    let occ: f64 = (lo..=hi).map(|k| (pa[k] - floor).max(0.0)).sum();
    let nk = (hi - lo + 1) as f64;
    let snr = db(occ / (floor * nk));
    let bw = nk * dfa;

    // the signal alone at baseband: the band and a margin (10 %, 20 Hz)
    let mg = (0.1 * bw + 20.0) / dfa;
    let (f0, f1) = (
        (lo as f64 - mg).max(1.0) as usize,
        ((hi as f64 + mg) as usize).min(kmax - 1),
    );
    let c = (centre / dfa).round() as i64;
    let mut yb = vec![Complex32::new(0.0, 0.0); nb];
    for k in f0..=f1 {
        let j = (k as i64 - c).rem_euclid(nb as i64) as usize;
        yb[j] = xa[k];
    }
    fft(&mut yb, true);
    let g = 1.0 / n as f32;
    let y: Vec<Complex32> = yb.iter().map(|v| v * g).collect();
    // the noise power that passes: (f1 - f0 + 1) bins of the floor
    let noise = (f1 - f0 + 1) as f64 * floor * (g as f64).powi(2);
    let mut m = Measure {
        lo: lo as f64 * dfa,
        hi: hi as f64 * dfa,
        centre,
        bw90,
        snr,
        // its extent over 90 % of the passband's middle
        fills: (hi - lo) as f64 > 0.9 * (b1 - b0) as f64 * 0.9,
        ..Default::default()
    };

    envelope(&mut m, &y, fs, noise, (f1 - f0 + 1) as f64 * dfa);
    tones(&mut m, &sp, floor, lo, hi, dfa, bw);
    carrier(&mut m, &pa, floor, lo, hi, dfa);
    grid(&mut m, &y, fs, bw);
    symbols(&mut m, &y, fs, bw);
    periods(&mut m, &y, fs, bw);
    drift(&mut m, &y, fs, bw);
    chirp(&mut m, &y, fs, noise);
    Some(m)
}

/// The noise floor of `x` (at `rate`) between bins `b0` and `b1` of its
/// whole-length FFT, as the mean of such a bin.
fn floor_of(x: &[Complex32], b0: usize, b1: usize, rate: f64) -> f64 {
    // frames of ~85, ~21 and ~5 ms, the lowest: a sweep crossing the
    // window lights all of a long frame, a short one falls between
    [0.085, 0.021, 0.0053]
        .iter()
        .filter_map(|&d| {
            let f = ((d * rate) as usize).next_power_of_two();
            (x.len() / f >= 8 && (b1 - b0) * f / x.len() >= 16).then(|| floor_frames(x, b0, b1, f))
        })
        .fold(f64::MAX, f64::min)
}

/// [`floor_of`] with frames of `f` samples.
fn floor_frames(x: &[Complex32], b0: usize, b1: usize, f: usize) -> f64 {
    let na = x.len();
    let r = na as f64 / f as f64;
    let (k0, k1) = (
        (b0 as f64 / r) as usize,
        ((b1 as f64 / r) as usize).min(f - 1),
    );
    let e = (k1 - k0) / 20;
    let (k0, k1) = (k0 + e, k1 - e);
    let w: Vec<f32> = (0..f)
        .map(|i| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / f as f64).cos()) as f32)
        .collect();
    let mut all = Vec::new();
    let mut p = FftPlanner::new();
    let plan = p.plan_fft_forward(f);
    let mut buf = vec![Complex32::new(0.0, 0.0); f];
    for c in x.chunks_exact(f) {
        for i in 0..f {
            buf[i] = c[i] * w[i];
        }
        plan.process(&mut buf);
        let pw: Vec<f64> = buf.iter().map(|v| v.norm_sqr() as f64).collect();
        let sm = smooth(&pw, 2);
        all.extend_from_slice(&sm[k0..=k1]);
    }
    // white noise of variance v: a frame's bin has mean v sum(w^2) =
    // 0.375 f v, a whole-length bin na v; FLOOR_P10: the 10th percentile
    // of the smoothed bins over their mean (measured: the test below)
    percentile(&all, 0.1) / FLOOR_P10 / (0.375 * f as f64) * na as f64
}

/// The 10th percentile of five neighbouring Hann-windowed bins of white
/// noise averaged, over their mean.
const FLOOR_P10: f64 = 0.373;

/// Keying, level variation, syllable-rate variation.
fn envelope(m: &mut Measure, y: &[Complex32], fs: f64, noise: f64, bw: f64) {
    // frames of 2.5 ms, smoothed to ~1/bw (bw: the filter's; what the
    // filter lets through changes no faster)
    let l = ((0.0025 * fs).round() as usize).max(1);
    let fr: Vec<f64> = y
        .chunks_exact(l)
        .map(|c| c.iter().map(|v| v.norm_sqr() as f64).sum::<f64>() / l as f64)
        .collect();
    let h = ((1.0 / bw.max(10.0)) * fs / l as f64 / 2.0).round() as usize;
    let fsm = smooth(&fr, h.min(16));
    let dof = ((2 * h + 1) * l) as f64 * (bw / fs).min(1.0);
    let p95 = percentile(&fsm, 0.95);
    // on: clearly over the noise, and within 10 dB of the top (keying
    // edges timed near half amplitude, not in the filter's tails)
    let thr = (noise * (1.0 + 4.0 / dof.max(1.0).sqrt())).max(0.1 * p95);
    let mut on: Vec<bool> = fsm.iter().map(|&v| v > thr).collect();
    // lone frames flipped (median of three)
    for i in 1..on.len() - 1 {
        if on[i - 1] == on[i + 1] {
            on[i] = on[i - 1];
        }
    }
    m.duty = on.iter().filter(|&&b| b).count() as f64 / on.len() as f64;
    let (onp, offp): (Vec<f64>, Vec<f64>) = {
        let a: Vec<f64> = fsm
            .iter()
            .zip(&on)
            .filter(|(_, o)| **o)
            .map(|(v, _)| *v)
            .collect();
        let b: Vec<f64> = fsm
            .iter()
            .zip(&on)
            .filter(|(_, o)| !**o)
            .map(|(v, _)| *v)
            .collect();
        (a, b)
    };
    if !onp.is_empty() && !offp.is_empty() {
        m.depth = db(percentile(&onp, 0.5) / percentile(&offp, 0.5).max(1e-30));
    }
    let mut runs = Vec::new();
    let mut i = 0;
    while i < on.len() {
        let j = (i..on.len()).find(|&j| on[j] != on[i]).unwrap_or(on.len());
        // runs cut by the window's ends don't say how long they are
        if on[i] && i > 0 && j < on.len() {
            runs.push((j - i) as f64 * l as f64 / fs);
        }
        i = j;
    }
    m.bursts = runs.len() as u32;
    if !runs.is_empty() {
        m.on_mean = runs.iter().sum::<f64>() / runs.len() as f64;
        m.on_short = percentile(&runs, 0.25);
    }
    // the amplitude while on, the noise taken out, in 100-ms blocks
    let amp: Vec<Option<f64>> = fr
        .iter()
        .zip(&on)
        .map(|(&v, &o)| o.then(|| (v - noise).max(0.0).sqrt()))
        .collect();
    let cvs: Vec<f64> = amp
        .chunks((0.1 * fs) as usize / l)
        .filter_map(|c| {
            let a: Vec<f64> = c.iter().flatten().copied().collect();
            if a.len() < c.len() * 3 / 4 {
                return None;
            }
            let mu = a.iter().sum::<f64>() / a.len() as f64;
            let var = a.iter().map(|x| (x - mu).powi(2)).sum::<f64>() / a.len() as f64;
            Some(var.sqrt() / mu.max(1e-30))
        })
        .collect();
    if cvs.len() >= 3 {
        m.env_cv = percentile(&cvs, 0.5);
    }
    // peak to average: the samples of the frames that are on
    let pw: Vec<f64> = y
        .chunks_exact(l)
        .zip(&on)
        .filter(|(_, o)| **o)
        .flat_map(|(c, _)| c.iter().map(|v| v.norm_sqr() as f64))
        .collect();
    if pw.len() > 1000 {
        let mean = pw.iter().sum::<f64>() / pw.len() as f64;
        let sig = (mean - noise).max(1e-30);
        m.papr = db(((percentile(&pw, 0.99) - noise).max(0.0) / sig).max(1e-3));
        // E|x|^4 = E|s|^4 + 4 S N + 2 N^2 (complex Gaussian noise of power N)
        let m4 = pw.iter().map(|p| p * p).sum::<f64>() / pw.len() as f64;
        if sig > 0.5 * noise {
            m.kurt = (m4 - 4.0 * sig * noise - 2.0 * noise * noise) / (sig * sig);
        }
    }
    // syllable rates: the envelope's (amplitude, 400 frames/s) spectrum
    let a: Vec<f64> = fr.iter().map(|v| v.sqrt()).collect();
    let mu = a.iter().sum::<f64>() / a.len() as f64;
    let mut z: Vec<Complex32> = a
        .iter()
        .map(|v| Complex32::new((v - mu) as f32, 0.0))
        .collect();
    fft(&mut z, false);
    let df = (fs / l as f64) / z.len() as f64;
    let (mut s, mut t) = (0.0, 0.0);
    for (k, v) in z[1..z.len() / 2].iter().enumerate() {
        let f = (k + 1) as f64 * df;
        let p = v.norm_sqr() as f64;
        if f <= 50.0 {
            t += p;
        }
        if (1.0..=10.0).contains(&f) {
            s += p;
        }
    }
    m.syllabic = s / t.max(1e-30);
}

/// Tones: peaks of the spectrum smoothed to the signal's scale with a
/// prominence of 6 dB.
fn tones(m: &mut Measure, sp: &[f64], floor: f64, lo: usize, hi: usize, dfa: f64, bw: f64) {
    let h = ((bw / 60.0).clamp(1.5, 10.0) / dfa).round() as usize;
    let (a, b) = (lo.saturating_sub(4 * h), (hi + 4 * h).min(sp.len() - 1));
    let s: Vec<f64> = smooth(&sp[a..=b], h).iter().map(|&v| db(v)).collect();
    let fl = db(floor);
    let mut peaks: Vec<(usize, f64)> = Vec::new();
    for i in 1..s.len() - 1 {
        if s[i] < s[i - 1] || s[i] < s[i + 1] || s[i] < fl + 6.0 {
            continue;
        }
        // prominence: down to the higher of the lowest points before a
        // higher peak on each side
        let side = |it: &mut dyn Iterator<Item = usize>| {
            let mut low = s[i];
            for j in it {
                if s[j] > s[i] {
                    return low;
                }
                low = low.min(s[j]);
            }
            low
        };
        let l = side(&mut (0..i).rev());
        let r = side(&mut (i + 1..s.len()));
        if s[i] - l.max(r) >= 6.0 {
            peaks.push((i, s[i]));
        }
    }
    // plateaus: one peak each
    peaks.dedup_by(|p, q| p.0 <= q.0 + 2 * h && (p.1 - q.1).abs() < 0.5);
    peaks.sort_by(|p, q| q.1.total_cmp(&p.1));
    peaks.truncate(64);
    m.tones = peaks
        .iter()
        .map(|&(i, v)| Line {
            at: (a + i) as f64 * dfa,
            db: v - fl,
        })
        .collect();
    // common spacing: the median gap of neighbours, if most gaps are a
    // whole multiple of it
    if m.tones.len() >= 3 {
        let mut f: Vec<f64> = m.tones.iter().map(|t| t.at).collect();
        f.sort_by(|a, b| a.total_cmp(b));
        let gaps: Vec<f64> = f.windows(2).map(|w| w[1] - w[0]).collect();
        let d = percentile(&gaps, 0.0).max(percentile(&gaps, 0.25) * 0.5);
        let d = {
            // refine on the gaps that are about one step
            let one: Vec<f64> = gaps
                .iter()
                .copied()
                .filter(|g| (g / d).round() == 1.0)
                .collect();
            if one.is_empty() {
                d
            } else {
                percentile(&one, 0.5)
            }
        };
        let fit = gaps
            .iter()
            .filter(|&&g| {
                let k = (g / d).round();
                k >= 1.0 && (g - k * d).abs() < 0.12 * d
            })
            .count();
        if d > 2.0 * dfa && fit as f64 >= 0.8 * gaps.len() as f64 {
            m.spacing = Some(d);
        }
    }
}

/// The strongest narrow line's share of the power; AM: sidebands on both
/// sides of it.
fn carrier(m: &mut Measure, pa: &[f64], floor: f64, lo: usize, hi: usize, dfa: f64) {
    let w = (3.0 / dfa).round() as usize;
    let ex: Vec<f64> = (0..pa.len()).map(|k| (pa[k] - floor).max(0.0)).collect();
    let tot: f64 = ex[lo..=hi].iter().sum();
    let sm = smooth(&ex[lo..=hi], w);
    let Some((k, _)) = sm.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)) else {
        return;
    };
    let k = lo + k;
    let (a, b) = (k.saturating_sub(w).max(lo), (k + w).min(hi));
    let lp: f64 = ex[a..=b].iter().sum();
    m.line = lp / tot.max(1e-30);
    m.line_at = k as f64 * dfa;
    // every narrow line with 5 % or more: peaks of the 6-Hz sums
    for j in w..sm.len().saturating_sub(w) {
        let v = sm[j];
        if v >= sm[j - 1]
            && v > sm[j + 1]
            && sm[j.saturating_sub(4 * w)..(j + 4 * w).min(sm.len())]
                .iter()
                .all(|&u| u <= v)
        {
            let (a, b) = (lo + j - w.min(j), (lo + j + w).min(hi));
            let share = ex[a..=b].iter().sum::<f64>() / tot.max(1e-30);
            if share >= 0.05 {
                m.carriers.push(Line {
                    at: (lo + j) as f64 * dfa,
                    db: share,
                });
            }
        }
    }
    m.carriers.sort_by(|a, b| b.db.total_cmp(&a.db));
    m.carriers.truncate(6);
    // sidebands: the power 20 Hz+ away below and above, in the band
    let g = (20.0 / dfa) as usize;
    let below: f64 = if k > lo + g {
        ex[lo..k - g].iter().sum()
    } else {
        0.0
    };
    let above: f64 = if k + g < hi {
        ex[k + g..=hi].iter().sum()
    } else {
        0.0
    };
    m.side_lo = below / tot.max(1e-30);
    m.side_hi = above / tot.max(1e-30);
    // mirrored: 20 Hz+ out on both sides, as far as both go, in 10-Hz steps
    let st = (10.0 / dfa).round().max(1.0) as usize;
    let reach = (k - lo).min(hi - k);
    let sm10 = smooth(&ex, st / 2);
    let (mut u, mut d) = (Vec::new(), Vec::new());
    let mut j = g;
    while j < reach {
        u.push((sm10[k + j] + floor * 0.1).ln());
        d.push((sm10[k - j] + floor * 0.1).ln());
        j += st;
    }
    if u.len() >= 8 {
        let n = u.len() as f64;
        let (mu, md) = (u.iter().sum::<f64>() / n, d.iter().sum::<f64>() / n);
        let (mut suu, mut sdd, mut sud) = (0.0, 0.0, 0.0);
        for (a, b) in u.iter().zip(&d) {
            suu += (a - mu).powi(2);
            sdd += (b - md).powi(2);
            sud += (a - mu) * (b - md);
        }
        m.mirror = sud / (suu * sdd).sqrt().max(1e-30);
    }
}

/// The tone grid, frame by frame: for frame lengths of 8 to 256 ms, each
/// frame's strongest frequency (where it stands well out of the frame);
/// their histogram's autocorrelation gives the spacing; the frame length
/// whose histogram is the most periodic wins, a grid that two frame
/// lengths agree on first.
fn grid(m: &mut Measure, y: &[Complex32], fs: f64, bw: f64) {
    let half = 0.55 * bw + 20.0;
    let mut cands: Vec<Grid> = Vec::new();
    for t in [0.008, 0.016, 0.032, 0.064, 0.128, 0.256] {
        let l = (t * fs) as usize;
        let nf = (4 * l).next_power_of_two();
        let df = fs / nf as f64;
        let kh = ((half / df) as usize).min(nf / 2 - 2);
        if kh < 8 || l < 8 {
            continue;
        }
        let w: Vec<f32> = (0..l)
            .map(|i| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / l as f64).cos()) as f32)
            .collect();
        let mut peaks = Vec::new();
        let mut buf = vec![Complex32::new(0.0, 0.0); nf];
        let mut a = 0;
        while a + l <= y.len() {
            buf.iter_mut().for_each(|v| *v = Complex32::new(0.0, 0.0));
            for i in 0..l {
                buf[i] = y[a + i] * w[i];
            }
            fft(&mut buf, false);
            let p: Vec<f64> = (0..2 * kh + 1)
                .map(|j| buf[(j + nf - kh) % nf].norm_sqr() as f64)
                .collect();
            let (j, &pk) = p
                .iter()
                .enumerate()
                .max_by(|x, y| x.1.total_cmp(y.1))
                .unwrap();
            let med = percentile(&p, 0.5);
            if pk > 8.0 * med && j > 0 && j + 1 < p.len() {
                let (u, v, z) = (p[j - 1].sqrt(), pk.sqrt(), p[j + 1].sqrt());
                let d = 0.5 * (u - z) / (u - 2.0 * v + z).min(-1e-30);
                peaks.push((j as f64 - kh as f64 + d.clamp(-0.5, 0.5)) * df);
            }
            a += l / 2;
        }
        if peaks.len() < 30 {
            continue;
        }
        // histogram, 0.25 Hz bins, smoothed over the frame's resolution
        const HB: f64 = 0.25;
        let nh = ((2.0 * half / HB) as usize).next_power_of_two() * 2;
        let mut h = vec![0f64; nh];
        for &f in &peaks {
            let k = ((f + half) / HB) as usize;
            if k < nh / 2 {
                h[k] += 1.0;
            }
        }
        let h = smooth(&h, ((0.25 / t / HB) as usize).max(1));
        let mut z: Vec<Complex32> = h.iter().map(|&v| Complex32::new(v as f32, 0.0)).collect();
        fft(&mut z, false);
        z.iter_mut()
            .for_each(|v| *v = Complex32::new(v.norm_sqr(), 0.0));
        fft(&mut z, true);
        let r0 = z[0].re.max(1e-30) as f64;
        let r: Vec<f64> = z[..nh / 2].iter().map(|v| v.re as f64 / r0).collect();
        // the first peak (from 3 Hz and the frame's resolution up) nearly
        // as high as the highest: the spacing, not a multiple of it
        let k0 = ((3.0f64).max(0.7 / t) / HB) as usize;
        let kmax = ((half / HB) as usize).min(r.len() - 2);
        if kmax <= k0 + 2 {
            continue;
        }
        let maxima: Vec<usize> = (k0.max(1)..kmax)
            .filter(|&k| r[k] >= r[k - 1] && r[k] > r[k + 1])
            .collect();
        let Some(&gm) = maxima.iter().max_by(|x, y| r[**x].total_cmp(&r[**y])) else {
            continue;
        };
        let kf = maxima
            .iter()
            .copied()
            .find(|&k| r[k] >= 0.6 * r[gm])
            .unwrap_or(gm);
        // a clear peak: a smooth histogram (frequencies all over, as a
        // single wide carrier gives) ripples, it doesn't repeat
        let dip = r[k0..kf].iter().copied().fold(f64::MAX, f64::min);
        if r[kf] - dip < 0.1 {
            continue;
        }
        let rf = r[kf];
        if rf < 0.25 {
            continue;
        }
        let sp0 = kf as f64 * HB;
        // the tones: the peaks grouped on that grid; the spacing refined by
        // a straight line through the groups' medians (weighted by their
        // frames); tones used: groups with 10 % of the busiest one's frames
        let base = peaks.iter().copied().fold(f64::MAX, f64::min);
        let mut groups: std::collections::BTreeMap<i64, Vec<f64>> = Default::default();
        for &f in &peaks {
            groups
                .entry(((f - base) / sp0).round() as i64)
                .or_default()
                .push(f);
        }
        let mx = groups.values().map(|g| g.len()).max().unwrap_or(0);
        let pts: Vec<(f64, f64, f64)> = groups
            .iter()
            .filter(|(_, g)| g.len() * 10 >= mx)
            .map(|(&i, g)| (i as f64, percentile(g, 0.5), g.len() as f64))
            .collect();
        let spacing = if pts.len() >= 2 {
            let w: f64 = pts.iter().map(|p| p.2).sum();
            let mi = pts.iter().map(|p| p.0 * p.2).sum::<f64>() / w;
            let mf = pts.iter().map(|p| p.1 * p.2).sum::<f64>() / w;
            let sii: f64 = pts.iter().map(|p| p.2 * (p.0 - mi).powi(2)).sum();
            let sif: f64 = pts.iter().map(|p| p.2 * (p.0 - mi) * (p.1 - mf)).sum();
            if sii > 0.0 { sif / sii } else { sp0 }
        } else {
            sp0
        };
        let cnt: std::collections::BTreeMap<i64, u32> =
            groups.iter().map(|(&i, g)| (i, g.len() as u32)).collect();
        let mx = cnt.values().copied().max().unwrap_or(0);
        let n = cnt.values().filter(|&&c| c * 10 >= mx).count() as u32;
        // how often the strongest tone moves to another
        let q: Vec<i64> = peaks
            .iter()
            .map(|f| ((f - base) / sp0).round() as i64)
            .collect();
        let changes =
            q.windows(2).filter(|w| w[0] != w[1]).count() as f64 / (q.len() as f64 * t / 2.0);
        cands.push(Grid {
            spacing,
            n,
            strength: rf,
            frame: t,
            changes,
        });
    }
    // a grid seen at another frame length too (within 4 %) is the grid; a
    // frame shorter than the symbols shows a wrong one on its own
    let agreed = |g: &Grid| {
        cands
            .iter()
            .any(|h| h.frame != g.frame && (h.spacing / g.spacing - 1.0).abs() < 0.04)
    };
    let score = |g: &Grid| g.strength + if agreed(g) { 0.15 } else { 0.0 };
    m.grid = cands
        .iter()
        .filter(|g| g.n >= 3)
        .max_by(|a, b| score(a).total_cmp(&score(b)))
        .copied();
}

/// Symbol rate and PSK order.
fn symbols(m: &mut Measure, y: &[Complex32], fs_: f64, bw: f64) {
    let top = (1.3 * bw).clamp(20.0, 0.48 * fs_);
    let nb = y.len();
    let df = fs_ / nb as f64;
    // the envelope
    let e: Vec<f64> = y.iter().map(|v| v.norm_sqr() as f64).collect();
    let ps = feature_spectrum(&e, nb);
    // (not where a carrier or speech moves it: a voice's pitch)
    m.baud_env = find_line(&ps, df, 8.0, top, 0.15 * top + 10.0)
        .filter(|l| l.db >= 14.0 && m.line < 0.5 && m.syllabic < 0.35);
    // frequency transitions. Two tones: a discriminator, as a demodulator
    // has (each tone's energy through a moving average of 1/shift, which
    // has its null on the other; (a - b) / (a + b): fading cancels out).
    // Otherwise the instantaneous frequency smoothed over ~1/(2 bw).
    // The transitions: squared differences over that span.
    let (fs, s) = if two_tones(&m.tones) && m.line < 0.5 {
        let (fa, fb) = (m.tones[0].at - m.centre, m.tones[1].at - m.centre);
        let l = ((fs_ / (fa - fb).abs()).round() as usize).clamp(2, 600);
        let energy = |f: f64| -> Vec<f64> {
            let w = -2.0 * std::f64::consts::PI * f / fs_;
            let mut cs = vec![num_complex::Complex64::new(0.0, 0.0); y.len() + 1];
            for (n, v) in y.iter().enumerate() {
                let z = num_complex::Complex64::new(v.re as f64, v.im as f64)
                    * num_complex::Complex64::from_polar(1.0, w * n as f64);
                cs[n + 1] = cs[n] + z;
            }
            (0..y.len() - l)
                .map(|n| (cs[n + l] - cs[n]).norm_sqr())
                .collect()
        };
        let (ea, eb) = (energy(fa), energy(fb));
        let d: Vec<f64> = ea
            .iter()
            .zip(&eb)
            .map(|(a, b)| (a - b) / (a + b + 1e-30))
            .collect();
        (d, l)
    } else {
        let s = ((fs_ / (2.0 * bw.max(20.0))).round() as usize).clamp(1, 60);
        let fi: Vec<f64> = y
            .windows(2)
            .map(|w| (w[1] * w[0].conj()).arg() as f64)
            .collect();
        (smooth(&fi, s / 2), s)
    };
    let t: Vec<f64> = (s..fs.len()).map(|i| (fs[i] - fs[i - s]).powi(2)).collect();
    let pt = feature_spectrum(&t, nb);
    // (async FSK puts its transitions on a half-bit grid when it has 1.5
    // stop bits, and the line can be at any multiple of the rate.) The bit
    // is the shortest common run of one tone (2-FSK): the line taken as
    // its nearest multiple.
    // (only for tones: FSK)
    let fsk = m.tones.len() >= 2 && m.line < 0.5 && (m.env_cv < 0.25 || two_tones(&m.tones));
    let line = find_line(&pt, df, 8.0, top, 0.15 * top + 10.0).filter(|l| fsk && l.db >= 14.0);
    let runs = tone_runs(&fs, s);
    let short = (runs.len() >= 20 && two_tones(&m.tones)).then(|| fs_ / percentile(&runs, 0.15));
    m.baud_freq = match (line, short) {
        (Some(l), Some(b0)) => {
            let n = (l.at / b0).round().max(1.0);
            if (l.at / n - b0).abs() < 0.2 * b0 {
                Some(Line {
                    at: l.at / n,
                    db: l.db,
                })
            } else {
                Some(l)
            }
        }
        (l, _) => l,
    };
    if let (Some(b), true) = (m.baud_freq, two_tones(&m.tones)) {
        let h = ((fs_ / b.at / 4.0).round() as usize).max(1);
        let tr = smooth(&fi_raw(y), h);
        let amp: Vec<f64> = smooth(&y.iter().map(|v| v.norm() as f64).collect::<Vec<_>>(), h);
        let strong = percentile(&amp, 0.5);
        let mid = 0.5 * (m.tones[0].at + m.tones[1].at) - m.centre;
        let rad = 2.0 * std::f64::consts::PI / fs_;
        let (mut up, mut dn) = (Vec::new(), Vec::new());
        for (i, &v) in tr.iter().enumerate() {
            if amp.get(i + 1).is_some_and(|&a| a >= strong) {
                let hz = v / rad;
                if hz > mid { up.push(hz) } else { dn.push(hz) }
            }
        }
        if up.len() > 50 && dn.len() > 50 {
            m.shift = Some(percentile(&up, 0.5) - percentile(&dn, 0.5));
        }
    }
    // PSK order: y^k shows a line (at k times the carrier offset); not a
    // carrier (y itself has one), nor constant-envelope tones (FSK's show
    // lines too)
    if m.tones.len() > 1 && m.env_cv < 0.2 {
        return;
    }
    let line_of = |k: i32| {
        let mut z: Vec<Complex32> = y
            .iter()
            .map(|v| {
                let a = v.norm();
                if a > 0.0 {
                    v.powi(k) / a.powi(k - 1)
                } else {
                    *v
                }
            })
            .collect();
        fft(&mut z, false);
        let p: Vec<f64> = z.iter().map(|c| c.norm_sqr() as f64).collect();
        // the peak's 3 bins: their share of the power
        let (i, _) = p
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        let n = p.len();
        let s3 = p[(i + n - 1) % n] + p[i] + p[(i + 1) % n];
        let tot: f64 = p.iter().sum();
        let f = if i < n / 2 {
            i as f64
        } else {
            i as f64 - n as f64
        } * df
            / k as f64;
        Line {
            at: f,
            db: s3 / tot.max(1e-30),
        }
    };
    // (the share of a line: a carrier ~1, noise-like ~3 / the bins it
    // spreads over)
    let l1 = line_of(1);
    if l1.db < 0.1 {
        for k in [2, 4, 8] {
            let l = line_of(k);
            if l.db >= 0.1 {
                m.psk = Some((k as u32, l));
                break;
            }
        }
    }
}

/// The instantaneous frequency (rad / sample).
fn fi_raw(y: &[Complex32]) -> Vec<f64> {
    y.windows(2)
        .map(|w| (w[1] * w[0].conj()).arg() as f64)
        .collect()
}

/// Two tones stand out: the strongest two within 12 dB of each other,
/// every other one 6 dB under the weaker or between them (FSK's
/// transitions).
fn two_tones(t: &[Line]) -> bool {
    t.len() >= 2
        && (t[0].db - t[1].db).abs() < 12.0
        && t[2..]
            .iter()
            .all(|x| x.db < t[0].db.min(t[1].db) - 6.0 || (x.at - t[0].at) * (x.at - t[1].at) < 0.0)
}

/// Lengths (samples) of the runs above and below the middle of a
/// frequency track, with hysteresis of a quarter of the spread; the
/// runs cut by the ends left out.
fn tone_runs(f: &[f64], skip: usize) -> Vec<f64> {
    let f = &f[skip.min(f.len())..];
    let (lo, hi) = (percentile(f, 0.1), percentile(f, 0.9));
    let mid = 0.5 * (lo + hi);
    let hy = 0.25 * (hi - lo);
    let mut out = Vec::new();
    let (mut state, mut start) = (f.first().is_some_and(|&v| v > mid), 0usize);
    let mut first = true;
    for (i, &v) in f.iter().enumerate() {
        let flip = if state { v < mid - hy } else { v > mid + hy };
        if flip {
            if !first {
                out.push((i - start) as f64);
            }
            first = false;
            state = !state;
            start = i;
        }
    }
    out
}

/// Repetition periods: of the signal (|R| of the complex baseband) and
/// of its envelope; from 5 ms to 1.5 s.
fn periods(m: &mut Measure, y: &[Complex32], fs: f64, bw: f64) {
    let n = y.len();
    let mut z = vec![Complex32::new(0.0, 0.0); 2 * n];
    z[..n].copy_from_slice(y);
    fft(&mut z, false);
    for v in z.iter_mut() {
        *v = Complex32::new(v.norm_sqr(), 0.0);
    }
    fft(&mut z, true);
    let r0 = z[0].re.max(1e-30) as f64;
    let max = (1.5 * fs) as usize;
    // |R| relative to lag 0, the overlap's shrinking undone
    let r: Vec<f64> = (0..max)
        .map(|l| z[l].norm() as f64 / r0 * n as f64 / (n - l) as f64)
        .collect();
    let l0 = ((0.005 * fs) as usize).max((3.0 * fs / bw.max(1.0)) as usize);
    // (tones and carriers beat: their |R| repeats at 1 / their spacing,
    // which is no structure)
    let mut gaps: Vec<f64> = m.spacing.into_iter().collect();
    let t3 = &m.tones[..m.tones.len().min(3)];
    for (i, a) in t3.iter().enumerate() {
        for b in &t3[i + 1..] {
            gaps.push((a.at - b.at).abs());
        }
    }
    let beat = |p: f64| {
        gaps.iter()
            .any(|d| (1..=4).any(|k| (p * d - k as f64).abs() < 0.05 * k as f64))
    };
    if m.line < 0.2 {
        m.period = first_period(&r, l0, fs).filter(|l| !beat(l.at));
        // narrow peaks (a few samples wide) over their surroundings
        let (a, b) = (
            ((0.002 * fs) as usize).max(l0),
            ((0.040 * fs) as usize).min(r.len() - 1),
        );
        let w = ((0.001 * fs) as usize).max(3);
        let mut best: Option<(f64, usize)> = None;
        for k in a.max(w + 1)..b.min(r.len() - w - 1) {
            if r[k] < r[k - 1] || r[k] < r[k + 1] {
                continue;
            }
            let around: Vec<f64> = (k - w..=k + w)
                .filter(|&j| j + 2 < k || j > k + 2)
                .map(|j| r[j])
                .collect();
            let base = percentile(&around, 0.5);
            let lift = r[k] - base;
            if lift > 0.04
                && r[k] > 2.5 * base
                && best.is_none_or(|(v, _)| lift > v)
                && !beat(k as f64 / fs)
            {
                best = Some((lift, k));
            }
        }
        m.symbol = best.map(|(_, k)| Line {
            at: k as f64 / fs,
            db: r[k],
        });
    }
    // envelope: 1 ms frames
    let l = ((0.001 * fs).round() as usize).max(1);
    let e: Vec<f64> = y
        .chunks_exact(l)
        .map(|c| c.iter().map(|v| v.norm_sqr() as f64).sum::<f64>())
        .collect();
    let mu = e.iter().sum::<f64>() / e.len() as f64;
    let ne = e.len();
    let mut z: Vec<Complex32> = e
        .iter()
        .map(|v| Complex32::new((v - mu) as f32, 0.0))
        .collect();
    z.resize(2 * ne, Complex32::new(0.0, 0.0));
    fft(&mut z, false);
    for v in z.iter_mut() {
        *v = Complex32::new(v.norm_sqr(), 0.0);
    }
    fft(&mut z, true);
    let r0 = z[0].re.max(1e-30) as f64;
    let max = (1.5 * fs / l as f64) as usize;
    let r: Vec<f64> = (0..max)
        .map(|l| z[l].re as f64 / r0 * ne as f64 / (ne - l) as f64)
        .collect();
    // (a steady envelope's variation is the noise's)
    if m.env_cv >= 0.1 || m.duty < 0.9 {
        m.env_period = first_period(&r, 2, fs / l as f64);
    }
}

/// Sweeps: bursts (the level over the noise, 0.25 ms smoothing), along
/// each one the instantaneous frequency fitted with a straight line.
fn chirp(m: &mut Measure, y: &[Complex32], fs: f64, noise: f64) {
    let h = ((0.000_125 * fs) as usize).max(1);
    let p = smooth(
        &y.iter().map(|v| v.norm_sqr() as f64).collect::<Vec<_>>(),
        h,
    );
    let thr = (8.0 * noise).max(0.2 * percentile(&p, 0.95));
    let fi = smooth(&fi_raw(y), h);
    let hz = fs / (2.0 * std::f64::consts::PI);
    let min = (0.0015 * fs) as usize;
    let mut fits: Vec<(f64, f64)> = Vec::new();
    let mut lens: Vec<f64> = Vec::new();
    // the time between flybacks within a run (a sawtooth inside the window)
    let mut periods: Vec<f64> = Vec::new();
    let mut runs = Vec::new();
    let mut i = 0;
    while i < fi.len() {
        if p[i] <= thr {
            i += 1;
            continue;
        }
        let a = i;
        while i < fi.len() && p[i] > thr {
            i += 1;
        }
        runs.push((a, i));
    }
    let d = 2 * h + 1;
    for (a, b) in runs {
        // flybacks: the frequency jumping by 40 % of its range at once
        let mut cuts = vec![a];
        if b > a + 4 * d {
            let r = &fi[a..b];
            let span = percentile(r, 0.95) - percentile(r, 0.05);
            let mut j = a;
            while j + d < b {
                if span > 0.0 && (fi[j + d] - fi[j]).abs() > 0.4 * span {
                    cuts.push(j + d / 2);
                    j += 4 * d;
                } else {
                    j += 1;
                }
            }
        }
        cuts.push(b);
        for w in cuts.windows(2) {
            let (s0, s1) = (w[0], w[1]);
            let whole = (s1 - s0) as f64 / fs;
            // the segment's middle (its edges ring), long enough to fit
            let (a, b) = (s0 + h * 2, s1.saturating_sub(h * 2));
            if b <= a + min {
                continue;
            }
            let n = (b - a) as f64;
            let (mt, mf) = ((a + b - 1) as f64 / 2.0, fi[a..b].iter().sum::<f64>() / n);
            let (mut stt, mut stf, mut sff) = (0.0, 0.0, 0.0);
            for (k, &f) in fi[a..b].iter().enumerate() {
                let dt = (a + k) as f64 - mt;
                stt += dt * dt;
                stf += dt * (f - mf);
                sff += (f - mf) * (f - mf);
            }
            let slope = stf / stt; // rad/sample per sample
            let r2 = if sff > 0.0 {
                stf * stf / (stt * sff)
            } else {
                0.0
            };
            if r2 >= 0.8 {
                fits.push((slope * hz * fs, r2));
                lens.push(whole);
            }
        }
        for w in cuts[1..cuts.len() - 1].windows(2) {
            periods.push((w[1] - w[0]) as f64 / fs);
        }
    }
    // one long straight glide (a sounder crossing once): a sweep too
    if let Some(k) = (0..fits.len())
        .filter(|&k| lens[k] >= 0.05 && fits[k].1 >= 0.95)
        .max_by(|&x, &y| lens[x].total_cmp(&lens[y]))
        && fits.len() < 3
        && fits[k].0.abs() >= 1000.0
    {
        m.chirp = Some(Chirp {
            slope: fits[k].0,
            n: 1,
            r2: fits[k].1,
            len: lens[k],
            rate: None,
        });
        return;
    }
    if fits.len() < 3 {
        return;
    }
    let s: Vec<f64> = fits.iter().map(|f| f.0).collect();
    let med = percentile(&s, 0.5);
    // the bursts that glide alike (within 15 % of the median)
    let alike: Vec<&(f64, f64)> = fits
        .iter()
        .filter(|f| (f.0 - med).abs() <= 0.15 * med.abs())
        .collect();
    let r2 = percentile(&alike.iter().map(|f| f.1).collect::<Vec<_>>(), 0.5);
    // a radar repeats (10+ sweeps a second: 30+ in a window) and its
    // sweeps are straight; a few short, rougher glides are a wideband
    // modem's phase noise (STANAG 4285, fax, parallel-tone modems)
    if alike.len() >= 8 && r2 >= 0.95 && med.abs() >= 1000.0 {
        let rate = (periods.len() >= 5).then(|| 1.0 / percentile(&periods, 0.5));
        m.chirp = Some(Chirp {
            slope: med,
            n: alike.len() as u32,
            r2,
            len: percentile(&lens, 0.5),
            rate,
        });
    }
}

/// Drift: the centroid in 0.5-s pieces, a straight line through them.
fn drift(m: &mut Measure, y: &[Complex32], fs: f64, bw: f64) {
    if m.duty < 0.6 || bw > 600.0 || m.snr < 6.0 {
        return;
    }
    let p = (0.5 * fs) as usize;
    let mut pts = Vec::new();
    for (i, c) in y.chunks_exact(p).enumerate() {
        let mut z = c.to_vec();
        fft(&mut z, false);
        let (mut s, mut w) = (0.0, 0.0);
        for (k, v) in z.iter().enumerate() {
            let f = if k < p / 2 {
                k as f64
            } else {
                k as f64 - p as f64
            } * fs
                / p as f64;
            if f.abs() > bw {
                continue;
            }
            let p = (v.norm_sqr() as f64).powi(2); // sharpen: the line, not the noise
            s += f * p;
            w += p;
        }
        if w > 0.0 {
            pts.push(((i as f64 + 0.5) * p as f64 / fs, s / w));
        }
    }
    if pts.len() < 6 {
        return;
    }
    let n = pts.len() as f64;
    let (mt, mf) = (
        pts.iter().map(|p| p.0).sum::<f64>() / n,
        pts.iter().map(|p| p.1).sum::<f64>() / n,
    );
    let stt: f64 = pts.iter().map(|p| (p.0 - mt).powi(2)).sum();
    let stf: f64 = pts.iter().map(|p| (p.0 - mt) * (p.1 - mf)).sum();
    let k = stf / stt;
    let res: f64 = pts
        .iter()
        .map(|p| (p.1 - mf - k * (p.0 - mt)).powi(2))
        .sum::<f64>()
        / n;
    // a steady drift: the line explains most of the movement
    let sff: f64 = pts.iter().map(|p| (p.1 - mf).powi(2)).sum::<f64>() / n;
    if k.abs() >= 0.3 && res < 0.3 * sff {
        m.drift = Some(k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    struct Rng(u64);
    impl Rng {
        fn u(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
        fn gauss(&mut self) -> f64 {
            let (a, b) = (self.u().max(1e-12), self.u());
            (-2.0 * a.ln()).sqrt() * (2.0 * PI * b).cos()
        }
        fn bit(&mut self) -> bool {
            self.u() < 0.5
        }
    }

    /// Audio from a frequency track and an amplitude track, plus noise
    /// (white, `n` rms; the passband 200-2800 Hz as the receiver's).
    fn audio(f: impl Fn(f64) -> f64, a: impl Fn(f64) -> f64, n: f64, seed: u64) -> Vec<f32> {
        let mut r = Rng(seed);
        let mut ph = 0.0;
        let mut x: Vec<f64> = (0..NA)
            .map(|i| {
                let t = i as f64 / AUDIO_RATE;
                ph += 2.0 * PI * f(t) / AUDIO_RATE;
                a(t) * ph.sin() + n * r.gauss()
            })
            .collect();
        // the passband
        let mut z: Vec<Complex32> = x.iter().map(|&v| Complex32::new(v as f32, 0.0)).collect();
        fft(&mut z, false);
        let df = AUDIO_RATE / NA as f64;
        for (k, v) in z.iter_mut().enumerate() {
            let f = (if k < NA / 2 { k } else { NA - k }) as f64 * df;
            if !(200.0..=2800.0).contains(&f) {
                *v = Complex32::new(0.0, 0.0);
            }
        }
        fft(&mut z, true);
        for (v, c) in x.iter_mut().zip(&z) {
            *v = c.re as f64 / NA as f64;
        }
        x.iter().map(|&v| v as f32).collect()
    }

    const BAND: (f64, f64) = (200.0, 2800.0);

    /// The floor of white noise is its level.
    #[test]
    fn floor_of_noise() {
        let x = audio(|_| 1000.0, |_| 0.0, 0.1, 11);
        let df = AUDIO_RATE / NA as f64;
        let (b0, b1) = ((200.0 / df) as usize, (2800.0 / df) as usize);
        let xc: Vec<Complex32> = x.iter().map(|&v| Complex32::new(v, 0.0)).collect();
        let fl = floor_of(&xc, b0, b1, AUDIO_RATE);
        let mut z: Vec<Complex32> = x.iter().map(|&v| Complex32::new(v, 0.0)).collect();
        fft(&mut z, false);
        let mean = z[b0..b1].iter().map(|c| c.norm_sqr() as f64).sum::<f64>() / (b1 - b0) as f64;
        eprintln!("floor/mean {}", fl / mean);
        assert!((fl / mean - 1.0).abs() < 0.1, "{}", fl / mean);
    }

    #[test]
    fn noise_alone_is_nothing() {
        let x = audio(|_| 1000.0, |_| 0.0, 0.1, 1);
        let m = measure(&x, (500.0, 2500.0), BAND);
        assert!(m.is_none(), "{m:?}");
    }

    #[test]
    fn carrier_and_drift() {
        let x = audio(|t| 1000.0 + 2.0 * t, |_| 0.3, 0.05, 2);
        let m = measure(&x, (900.0, 1100.0), BAND).unwrap();
        assert!((m.centre - 1004.0).abs() < 5.0, "{m:?}");
        assert!(m.bw90 < 20.0 && m.hi - m.lo < 100.0, "{m:?}");
        assert!(m.line > 0.6, "{m:?}");
        assert!(m.duty > 0.95 && m.env_cv < 0.15, "{m:?}");
        assert!(m.psk.is_none(), "{m:?}");
        let d = m.drift.expect("drift");
        assert!((d - 2.0).abs() < 0.3, "{d}");
    }

    #[test]
    fn rtty() {
        // 45.45 Bd, 170 Hz, async (1.5 stop bits), random characters
        let mut r = Rng(3);
        let mut halves = Vec::new();
        while halves.len() < 1000 {
            halves.extend([0, 0]);
            for _ in 0..5 {
                let b = r.bit() as u8;
                halves.extend([b, b]);
            }
            halves.extend([1, 1, 1]);
        }
        let x = audio(
            |t| {
                1500.0
                    + if halves[(t * 2.0 * 45.45) as usize] == 1 {
                        85.0
                    } else {
                        -85.0
                    }
            },
            |_| 0.3,
            0.05,
            4,
        );
        let m = measure(&x, (1200.0, 1800.0), BAND).unwrap();
        assert!((m.centre - 1500.0).abs() < 15.0, "{m:?}");
        assert!(m.env_cv < 0.2 && m.duty > 0.95 && m.kurt < 1.15, "{m:?}");
        let b = m.baud_freq.expect("baud from frequency");
        assert!((b.at - 45.45).abs() < 0.5, "{m:?}");
        let sh = m.shift.expect("shift");
        assert!((sh - 170.0).abs() < 5.0, "{m:?}");
    }

    #[test]
    fn bpsk31() {
        // PSK31: a phase reversal for a 0 (cosine-shaped), random bits
        let mut r = Rng(5);
        let bits: Vec<bool> = (0..200).map(|_| r.bit()).collect();
        let n = NA;
        let sps = AUDIO_RATE / 31.25;
        let mut sign = vec![1.0; bits.len() + 1];
        for i in 0..bits.len() {
            sign[i + 1] = if bits[i] { sign[i] } else { -sign[i] };
        }
        let mut x: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f64 / sps;
                let k = t as usize;
                let u = t - k as f64;
                let a = if sign[k] == sign[k + 1] {
                    sign[k]
                } else {
                    sign[k] * (PI * u).cos()
                };
                (0.3 * a * (2.0 * PI * 1000.0 * i as f64 / AUDIO_RATE).sin()) as f32
            })
            .collect();
        let noise = audio(|_| 1.0, |_| 0.0, 0.03, 6);
        for (a, b) in x.iter_mut().zip(&noise) {
            *a += b;
        }
        let m = measure(&x, (900.0, 1100.0), BAND).unwrap();
        assert!((m.centre - 1000.0).abs() < 5.0, "{m:?}");
        let b = m.baud_env.expect("baud from the envelope");
        assert!((b.at - 31.25).abs() < 0.3, "{b:?}");
        assert_eq!(m.psk.map(|p| p.0), Some(2), "{m:?}");
    }

    #[test]
    fn cw_keying() {
        // dits and dahs at 20 WPM (60 ms), random
        let mut r = Rng(7);
        let mut key = Vec::new();
        while key.len() < 120 {
            let d = if r.bit() { 3 } else { 1 };
            key.extend(std::iter::repeat_n(true, d));
            key.extend(std::iter::repeat_n(false, if r.u() < 0.3 { 3 } else { 1 }));
        }
        let x = audio(
            |_| 700.0,
            |t| if key[(t / 0.06) as usize] { 0.3 } else { 0.0 },
            0.03,
            8,
        );
        let m = measure(&x, (500.0, 900.0), BAND).unwrap();
        assert!(m.duty > 0.3 && m.duty < 0.8, "{m:?}");
        assert!(m.bursts > 15, "{m:?}");
        assert!((m.on_short - 0.06).abs() < 0.015, "{m:?}");
        assert!(m.depth > 20.0, "{m:?}");
    }

    #[test]
    fn am_with_carrier() {
        // a carrier at 1500 Hz with 400 and 700 Hz tones as sidebands
        let x = audio(
            |_| 1500.0,
            |t| {
                0.3 * (1.0
                    + 0.4 * (2.0 * PI * 400.0 * t).sin()
                    + 0.3 * (2.0 * PI * 700.0 * t).sin())
            },
            0.02,
            9,
        );
        let m = measure(&x, (500.0, 2500.0), BAND).unwrap();
        assert!((m.line_at - 1500.0).abs() < 2.0, "{m:?}");
        assert!(
            m.side_lo > 0.03 && (m.side_lo / m.side_hi - 1.0).abs() < 0.2,
            "{m:?}"
        );
        let mut f: Vec<f64> = m.tones.iter().map(|t| t.at).collect();
        f.sort_by(|a, b| a.total_cmp(b));
        assert_eq!(f.len(), 5, "{m:?}");
    }

    #[test]
    fn pulses_repeat() {
        // a 1 kHz carrier pulsed 5 ms every 25 ms (40 Hz)
        let x = audio(
            |_| 1000.0,
            |t| if (t * 40.0).fract() < 0.2 { 0.3 } else { 0.0 },
            0.02,
            10,
        );
        let m = measure(&x, (800.0, 1200.0), BAND).unwrap();
        let p = m.env_period.expect("pulse period");
        assert!((p.at - 0.025).abs() < 0.001, "{p:?}");
    }
    /// Complex baseband at 24 kS/s: a signal and noise; `f(t)`, `a(t)` as
    /// for [`audio`], `n` the noise per I and Q.
    fn iq(f: impl Fn(f64) -> f64, a: impl Fn(f64) -> f64, n: f64, seed: u64) -> Vec<Complex32> {
        let rate = 24_000.0;
        let mut r = Rng(seed);
        let mut ph = 0.0;
        (0..iq_len(rate))
            .map(|i| {
                let t = i as f64 / rate;
                ph += 2.0 * PI * f(t) / rate;
                Complex32::new(
                    (a(t) * ph.cos() + n * r.gauss()) as f32,
                    (a(t) * ph.sin() + n * r.gauss()) as f32,
                )
            })
            .collect()
    }

    /// IQ: frequencies from the centre, below it too; RTTY's figures as in
    /// audio.
    #[test]
    fn iq_tone_and_rtty() {
        let band = (-10_000.0, 10_000.0);
        let x = iq(|_| -4321.0, |_| 0.3, 0.02, 21);
        let m = measure_iq(&x, 24_000.0, (-4500.0, -4100.0), band).unwrap();
        assert!((m.centre + 4321.0).abs() < 2.0 && m.line > 0.6, "{m:?}");
        let mut r = Rng(22);
        let bits: Vec<bool> = (0..400).map(|_| r.bit()).collect();
        let x = iq(
            |t| {
                3000.0
                    + if bits[(t * 50.0) as usize] {
                        225.0
                    } else {
                        -225.0
                    }
            },
            |_| 0.3,
            0.02,
            23,
        );
        let m = measure_iq(&x, 24_000.0, (2500.0, 3500.0), band).unwrap();
        assert!((m.centre - 3000.0).abs() < 20.0, "{m:?}");
        let sh = m.shift.expect("shift");
        assert!((sh - 450.0).abs() < 8.0, "{m:?}");
        let b = m.baud_freq.expect("rate");
        assert!((b.at - 50.0).abs() < 0.5, "{b:?}");
    }

    /// OFDM (as DRM mode B: 206 carriers 46.875 Hz apart, 21.33 ms useful
    /// symbols, a 5.33 ms guard copied from their ends): 9.7 kHz wide,
    /// its useful symbol length in |R|.
    #[test]
    fn iq_ofdm_guard() {
        let rate = 24_000.0;
        let (tu, tg) = ((0.021_333 * rate) as usize, (0.005_333 * rate) as usize);
        let k = 206;
        let mut r = Rng(31);
        let n = iq_len(rate);
        let mut x = Vec::with_capacity(n + tu + tg);
        let mut buf = vec![Complex32::new(0.0, 0.0); tu];
        while x.len() < n {
            for v in buf.iter_mut() {
                *v = Complex32::new(0.0, 0.0);
            }
            for c in 0..k {
                let bin = (c as i64 - k as i64 / 2).rem_euclid(tu as i64) as usize;
                let ph = (r.u() * 4.0).floor() * PI / 2.0 + PI / 4.0;
                buf[bin] = Complex32::from_polar(1.0, ph as f32);
            }
            fft(&mut buf, true);
            x.extend_from_slice(&buf[tu - tg..]);
            x.extend_from_slice(&buf);
        }
        x.truncate(n);
        let p = x.iter().map(|v| v.norm_sqr()).sum::<f32>() / n as f32;
        let x: Vec<Complex32> = x
            .iter()
            .map(|v| {
                v / p.sqrt() * 0.3
                    + Complex32::new(0.02 * r.gauss() as f32, 0.02 * r.gauss() as f32)
            })
            .collect();
        let m = measure_iq(&x, rate, (-6000.0, 6000.0), (-10_000.0, 10_000.0)).unwrap();
        assert!((m.hi - m.lo - 9700.0).abs() < 600.0, "{m:?}");
        assert!(m.line < 0.05, "{m:?}");
        assert!(m.papr > 5.5 && (m.kurt - 2.0).abs() < 0.3, "{m:?}");
        let p = m.symbol.expect("the useful symbol in |R|");
        assert!((p.at - 0.021_333).abs() < 0.0005, "{p:?}");
    }
    /// A sweep through the window: OTH-SW-like FMCW, 60 kHz wide, 43
    /// sweeps a second (2.58 MHz/s): its slope, and its repetition from the
    /// envelope.
    #[test]
    fn iq_fmcw_sweep() {
        let (rate, w, sr) = (24_000.0, 60_000.0, 43.0);
        // (the receiver passes only its ±10 kHz of the sweep)
        let f = |t: f64| ((t * sr).fract() - 0.5) * w;
        let x = iq(
            f,
            |t| if f(t).abs() < 9_500.0 { 0.3 } else { 0.0 },
            0.01,
            41,
        );
        let m = measure_iq(&x, rate, (-10_000.0, 10_000.0), (-10_000.0, 10_000.0)).unwrap();
        let c = m.chirp.expect("a sweep");
        assert!(
            (c.slope / (w * sr) - 1.0).abs() < 0.05 && c.n >= 50,
            "{c:?}"
        );
        let p = m.env_period.expect("sweeps repeating");
        assert!((1.0 / p.at - sr).abs() < 1.5, "{p:?}");
    }

    /// A chirp sounder crossing the window once (100 kHz/s): one sweep.
    #[test]
    fn iq_single_sweep() {
        let f = |t: f64| -30_000.0 + 100_000.0 * (t - 0.5);
        let x = iq(
            f,
            |t| if f(t).abs() < 9_000.0 { 0.3 } else { 0.0 },
            0.01,
            43,
        );
        let m = measure_iq(&x, 24_000.0, (-10_000.0, 10_000.0), (-10_000.0, 10_000.0)).unwrap();
        let c = m.chirp.expect("a sweep");
        assert!(
            c.n == 1 && (c.slope / 100_000.0 - 1.0).abs() < 0.05 && c.len > 0.15,
            "{c:?}"
        );
    }

    /// A sweep narrower than the window (Foghorn-like: 10 kHz, 67 a
    /// second): always on, a sawtooth; its sweeps from the flybacks.
    #[test]
    fn iq_sawtooth_sweep() {
        let (w, sr) = (10_000.0, 67.0);
        let x = iq(|t| ((t * sr).fract() - 0.5) * w, |_| 0.3, 0.01, 47);
        let m = measure_iq(&x, 24_000.0, (-9_000.0, 9_000.0), (-10_000.0, 10_000.0)).unwrap();
        let c = m.chirp.expect("a sweep");
        assert!((c.slope / (w * sr) - 1.0).abs() < 0.05, "{c:?}");
        let r = c.rate.expect("its rate");
        assert!((r - sr).abs() < 2.0, "{c:?}");
    }
}
