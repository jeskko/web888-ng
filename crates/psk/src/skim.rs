// SPDX-License-Identifier: GPL-3.0-or-later
//! The passband skimmer: every BPSK signal (PSK31/63/125) in the audio is
//! found, its carrier and rate worked out, and decoded.
//!
//! * Every 2 s the last 4 s are searched: peaks of the averaged spectrum
//!   (2.9 Hz lines) at least 8 dB over the floor, at most 400 Hz wide.
//! * A peak is BPSK when the squared signal has one carrier line (twice
//!   the carrier: squaring removes the ±1 modulation; FSK squares to a
//!   line per tone), which also gives the carrier to a fraction of a
//!   hertz; its envelope's line at 31.25, 62.5 or 125 Hz gives the rate.
//!   Peaks that aren't are left alone for 30 s.
//! * A signal gets a channel at 16 samples a symbol and a [`Demod`],
//!   primed from the last 8 s; it is kept if [`MIN_BLOCKS`] blocks of 32
//!   symbols there read as clean BPSK (coherent, clean, with reversals:
//!   FSK and keyed carriers fail one or another). One silent (squelch closed, no text) for 20 s is
//!   dropped.

use crate::demod::{Demod, SPS};
use num_complex::{Complex32, Complex64};
use rustfft::FftPlanner;
use std::collections::VecDeque;
use web888_cw::dsp;

/// The input audio rate.
pub const AUDIO_RATE: f64 = 12_000.0;
/// The rates tried (Bd).
pub const RATES: [f64; 3] = [31.25, 62.5, 125.0];
const HISTORY_S: f64 = 8.0;
const SEARCH_S: f64 = 4.0;
const LINE_HZ: f64 = 12_000.0 / 4096.0;
/// A signal is kept when this many blocks of 32 symbols of its history
/// read as clean BPSK ([`Demod::bpsk_blocks`]).
pub const MIN_BLOCKS: u32 = 2;
/// A character with a bit whose phase change came within this cosine of
/// 90° is reported weak.
pub const WEAK: f32 = 0.5;

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A signal found: its carrier (audio Hz, or offset in IQ) and rate.
    Lock { id: u32, hz: f64, baud: f64 },
    /// New text (`t`: stream time of its first character); `weak` per
    /// character: it may have read otherwise.
    Text {
        id: u32,
        hz: f64,
        text: String,
        t: f64,
        weak: Vec<bool>,
    },
}

/// A signal being decoded.
#[derive(Clone, Debug, PartialEq)]
pub struct Active {
    pub id: u32,
    pub hz: f64,
    pub baud: f64,
    /// Average |cos| of the phase changes: ~1 clean, 0.64 noise.
    pub quality: f32,
    /// Squelch open (text being decoded).
    pub open: bool,
}

/// The input mixed from `hz` and decimated by `decim` (FIR to ±half_bw).
struct Channel {
    decim: usize,
    taps: Vec<f32>,
    ring: Vec<Complex32>,
    pos: usize,
    rot: Complex64,
    step: Complex64,
    count: usize,
    since_norm: usize,
}

impl Channel {
    fn new(hz: f64, half_bw: f64, rate: f64, out_rate: f64, taps: usize) -> Self {
        let n = taps | 1;
        let a = -2.0 * std::f64::consts::PI * hz / rate;
        Channel {
            decim: (rate / out_rate).round().max(1.0) as usize,
            taps: dsp::firwin(n, half_bw, rate),
            ring: vec![Complex32::new(0.0, 0.0); 2 * n],
            pos: 0,
            rot: Complex64::new(1.0, 0.0),
            step: Complex64::new(a.cos(), a.sin()),
            count: 0,
            since_norm: 0,
        }
    }

    fn delay(&self, rate: f64) -> f64 {
        (self.taps.len() - 1) as f64 / 2.0 / rate
    }

    fn push(&mut self, input: impl IntoIterator<Item = Complex32>, out: &mut Vec<Complex32>) {
        let n = self.taps.len();
        for x in input {
            let v = x * Complex32::new(self.rot.re as f32, self.rot.im as f32);
            self.rot *= self.step;
            self.since_norm += 1;
            if self.since_norm == 1024 {
                self.since_norm = 0;
                self.rot /= self.rot.norm();
            }
            self.ring[self.pos] = v;
            self.ring[self.pos + n] = v;
            self.pos = (self.pos + 1) % n;
            self.count += 1;
            if self.count == self.decim {
                self.count = 0;
                let mut acc = Complex32::new(0.0, 0.0);
                for (x, h) in self.ring[self.pos..self.pos + n].iter().zip(&self.taps) {
                    acc += x * *h;
                }
                out.push(acc);
            }
        }
    }
}

struct Sig {
    id: u32,
    /// The channel's centre (the carrier when found).
    off: f64,
    ch: Channel,
    dem: Demod,
    buf: Vec<Complex32>,
    born: f64,
}

impl Sig {
    fn hz(&self) -> f64 {
        self.off + self.dem.freq
    }
}

pub struct Skimmer {
    rate: f64,
    complex: bool,
    lo: f64,
    hi: f64,
    n_in: u64,
    hist: VecDeque<Complex32>,
    sigs: Vec<Sig>,
    ignore: Vec<(f64, f64)>,
    next_id: u32,
    next_run: u64,
    events: Vec<Event>,
    pub runs: u64,
    pub every: f64,
    pub max_signals: usize,
    /// Peaks this far over the floor are tried (dB).
    pub min_db: f32,
    planner: FftPlanner<f32>,
}

impl Default for Skimmer {
    fn default() -> Self {
        Self::new(200.0, 2800.0)
    }
}

impl Skimmer {
    /// Signals between `lo` and `hi` Hz of 12 kS/s audio.
    pub fn new(lo: f64, hi: f64) -> Self {
        Self::with_rate(AUDIO_RATE, false, lo, hi)
    }

    /// Signals between `lo` and `hi` Hz (offsets) of complex IQ at `rate`
    /// (a multiple of 2 kS/s, e.g. a 60 kS/s bin).
    pub fn new_iq(rate: f64, lo: f64, hi: f64) -> Self {
        let mut s = Self::with_rate(rate, true, lo, hi);
        s.max_signals = 48;
        s
    }

    fn with_rate(rate: f64, complex: bool, lo: f64, hi: f64) -> Self {
        let mut s = Skimmer {
            rate,
            complex,
            lo,
            hi,
            n_in: 0,
            hist: VecDeque::new(),
            sigs: Vec::new(),
            ignore: Vec::new(),
            next_id: 1,
            next_run: (2.0 * rate) as u64,
            events: Vec::new(),
            runs: 0,
            every: 2.0,
            max_signals: 16,
            min_db: 8.0,
            planner: FftPlanner::new(),
        };
        s.set_band(lo, hi);
        s
    }

    /// The band searched (audio Hz, or IQ offsets).
    pub fn set_band(&mut self, lo: f64, hi: f64) {
        let edge = self.rate / 2.0 - 100.0;
        self.lo = if self.complex {
            lo.max(-edge)
        } else {
            lo.max(50.0)
        };
        self.hi = hi.min(edge).max(self.lo + 100.0);
        let (a, b) = (self.lo, self.hi);
        self.sigs.retain(|s| (a..=b).contains(&s.hz()));
    }

    pub fn now(&self) -> f64 {
        self.n_in as f64 / self.rate
    }

    /// Feed real audio (a skimmer from [`Skimmer::new`]).
    pub fn push(&mut self, audio: &[f32]) {
        let iq: Vec<Complex32> = audio
            .iter()
            .map(|&a| Complex32::new(2.0 * a, 0.0))
            .collect();
        self.push_iq(&iq);
    }

    /// Feed complex IQ (a skimmer from [`Skimmer::new_iq`]).
    pub fn push_iq(&mut self, iq: &[Complex32]) {
        let mut ev = Vec::new();
        for s in &mut self.sigs {
            s.buf.clear();
            s.ch.push(iq.iter().copied(), &mut s.buf);
            let items = s.dem.feed(&s.buf);
            emit(s, items, &mut ev);
        }
        self.events.extend(ev);
        self.hist.extend(iq);
        let hk = (HISTORY_S * self.rate) as usize;
        if self.hist.len() > hk {
            let cut = self.hist.len() - hk;
            self.hist.drain(..cut);
        }
        self.n_in += iq.len() as u64;
        if self.n_in >= self.next_run {
            self.next_run += (self.every * self.rate) as u64;
            self.runs += 1;
            self.manage();
            self.detect();
        }
    }

    pub fn poll(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// The signals being decoded, by frequency.
    pub fn active(&self) -> Vec<Active> {
        let mut v: Vec<Active> = self
            .sigs
            .iter()
            .map(|s| Active {
                id: s.id,
                hz: s.hz(),
                baud: s.dem.baud,
                quality: s.dem.quality,
                open: s.dem.open,
            })
            .collect();
        v.sort_by(|a, b| a.hz.total_cmp(&b.hz));
        v
    }

    fn manage(&mut self) {
        let now = self.now();
        self.ignore.retain(|&(_, u)| u > now);
        // silent ones, and the younger of two that converged
        let mut drop = vec![false; self.sigs.len()];
        for (i, s) in self.sigs.iter().enumerate() {
            let heard = s.dem.last_text.max(s.born);
            if !s.dem.open && now - heard > 20.0 {
                drop[i] = true;
            }
            for (j, o) in self.sigs.iter().enumerate().take(i) {
                if !drop[j] && (s.hz() - o.hz()).abs() < 0.5 * s.dem.baud.min(o.dem.baud) {
                    drop[i] = true;
                }
            }
        }
        let mut k = 0;
        self.sigs.retain(|_| {
            k += 1;
            !drop[k - 1]
        });
    }

    fn detect(&mut self) {
        let need = (SEARCH_S * self.rate) as usize;
        if self.sigs.len() >= self.max_signals || self.hist.len() < need {
            return;
        }
        let x: Vec<Complex32> = self
            .hist
            .iter()
            .skip(self.hist.len() - need)
            .copied()
            .collect();
        // averaged spectrum, 2.9 Hz lines
        let nf = (self.rate / LINE_HZ).round() as usize;
        let df = self.rate / nf as f64;
        let fft = self.planner.plan_fft_forward(nf);
        let win: Vec<f32> = (0..nf)
            .map(|i| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / nf as f64).cos()) as f32)
            .collect();
        let k0 = (self.lo / df).ceil() as i64;
        let nb = ((self.hi / df).floor() as i64 - k0 + 1).max(0) as usize;
        if nb < 16 {
            return;
        }
        let mut p = vec![0f32; nb];
        let mut i = 0;
        while i + nf <= x.len() {
            let mut b: Vec<Complex32> = x[i..i + nf].iter().zip(&win).map(|(v, w)| v * w).collect();
            fft.process(&mut b);
            for (j, v) in p.iter_mut().enumerate() {
                *v += b[(k0 + j as i64).rem_euclid(nf as i64) as usize].norm_sqr();
            }
            i += nf / 2;
        }
        let db: Vec<f32> = p.iter().map(|v| 10.0 * (v + 1e-20).log10()).collect();
        let sm = dsp::smooth(&db, 3);
        let floor = dsp::percentile_filter(&sm, 30.0, ((600.0 / df) as usize | 1).max(3));
        let f = |j: usize| (k0 + j as i64) as f64 * df;
        // peaks, strongest first
        let mut peaks: Vec<(usize, f32)> = (1..nb - 1)
            .filter(|&j| sm[j] >= sm[j - 1] && sm[j] > sm[j + 1] && sm[j] - floor[j] >= self.min_db)
            .map(|j| (j, sm[j] - floor[j]))
            .collect();
        peaks.sort_by(|a, b| b.1.total_cmp(&a.1));
        let now = self.now();
        for (j, _) in peaks {
            if self.sigs.len() >= self.max_signals {
                break;
            }
            // the region over floor + 3 dB
            let (mut a, mut b) = (j, j);
            while a > 0 && sm[a - 1] - floor[a - 1] > 3.0 {
                a -= 1;
            }
            while b + 1 < nb && sm[b + 1] - floor[b + 1] > 3.0 {
                b += 1;
            }
            let width = f(b) - f(a) + df;
            let c = f(j);
            if width > 400.0
                || self.ignore.iter().any(|&(h, _)| (h - c).abs() < 40.0)
                || self
                    .sigs
                    .iter()
                    .any(|s| (s.hz() - c).abs() < (1.5 * s.dem.baud).max(40.0))
            {
                continue;
            }
            match self.confirm(&x, c) {
                Some((fc, baud))
                    if !self
                        .sigs
                        .iter()
                        .any(|s| (s.hz() - fc).abs() < s.dem.baud.max(baud)) =>
                {
                    self.lock(fc, baud);
                }
                Some(_) => {}
                None => self.ignore.push((c, now + 30.0)),
            }
        }
    }

    /// Is there BPSK near `c` in x? Its carrier and rate.
    fn confirm(&mut self, x: &[Complex32], c: f64) -> Option<(f64, f64)> {
        let out = 2000.0;
        let taps = ((self.rate / 2000.0) * 40.0) as usize;
        let mut ch = Channel::new(c, 250.0, self.rate, out, taps);
        let mut z = Vec::new();
        ch.push(x.iter().copied(), &mut z);
        let skip = (taps / ch.decim).min(z.len());
        let z = &z[skip..];
        if z.len() < 4000 {
            return None;
        }
        // the squared signal's line: twice the carrier offset
        let n = z.len().next_power_of_two() * 2;
        let fft = self.planner.plan_fft_forward(n);
        let mut s: Vec<Complex32> = z.iter().map(|v| v * v).collect();
        s.resize(n, Complex32::new(0.0, 0.0));
        fft.process(&mut s);
        let bin = out / n as f64;
        let span = (170.0 / bin) as i64; // ±85 Hz of carrier: ±170 squared
        let pw = |k: i64| s[k.rem_euclid(n as i64) as usize].norm_sqr();
        let (kmax, pmax) = (-span..=span)
            .map(|k| (k, pw(k)))
            .max_by(|a, b| a.1.total_cmp(&b.1))?;
        let around: Vec<f32> = (-span..=span).map(pw).collect();
        let med = dsp::median(&around).max(1e-30);
        if 10.0 * (pmax / med).log10() < 15.0 {
            return None;
        }
        // BPSK squares to one line; FSK (FT8, RTTY) to one per tone: the
        // next strongest, 4 Hz or more away, must be 8 dB down
        // (over the whole channel, ±250 Hz: RTTY's tones are 170 Hz apart)
        let guard = (4.0 / bin) as i64;
        let wide = (500.0 / bin) as i64;
        let second = (-wide..=wide)
            .filter(|k| (k - kmax).abs() > guard)
            .map(pw)
            .fold(0.0, f32::max);
        if pmax < 6.3 * second {
            return None;
        }
        let off = kmax as f64 * bin / 2.0;
        // the envelope's symbol rate line, at the carrier
        let w = -2.0 * std::f64::consts::PI * off / out;
        let mut e: Vec<Complex32> = z
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let r = Complex64::from_polar(1.0, w * i as f64);
                Complex32::new(
                    (v * Complex32::new(r.re as f32, r.im as f32)).norm_sqr(),
                    0.0,
                )
            })
            .collect();
        let mean = e.iter().map(|v| v.re).sum::<f32>() / e.len() as f32;
        for v in &mut e {
            v.re -= mean;
        }
        e.resize(n, Complex32::new(0.0, 0.0));
        fft.process(&mut e);
        let line = |hz: f64| {
            let k = (hz / bin).round() as i64;
            let peak = (k - 2..=k + 2)
                .map(|q| e[q as usize].norm_sqr())
                .fold(0.0, f32::max);
            let ref_: Vec<f32> = (k + 8..k + 60)
                .chain(k - 60..k - 8)
                .map(|q| e[q as usize].norm_sqr())
                .collect();
            10.0 * (peak / dsp::median(&ref_).max(1e-30)).log10()
        };
        let (baud, snr) = RATES
            .iter()
            .map(|&r| (r, line(r)))
            .max_by(|a, b| a.1.total_cmp(&b.1))?;
        (snr >= 8.0).then_some((c + off, baud))
    }

    /// Decodes it from the history; keeps it if it read as clean BPSK.
    fn lock(&mut self, fc: f64, baud: f64) {
        let out = baud * SPS as f64;
        let taps = ((self.rate / baud) * 1.2) as usize;
        let mut ch = Channel::new(fc, 2.0 * baud, self.rate, out, taps);
        let n0 = self.n_in - self.hist.len() as u64;
        let t0 = (n0 + ch.decim as u64 - 1) as f64 / self.rate - ch.delay(self.rate);
        let mut dem = Demod::new(baud, t0);
        let mut buf = Vec::new();
        ch.push(self.hist.iter().copied(), &mut buf);
        let items = dem.feed(&buf);
        if dem.bpsk_blocks() < MIN_BLOCKS {
            self.ignore.push((fc, self.now() + 30.0));
            return;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.events.push(Event::Lock { id, hz: fc, baud });
        let mut s = Sig {
            id,
            off: fc,
            ch,
            dem,
            buf: Vec::new(),
            born: self.now(),
        };
        let mut ev = Vec::new();
        emit(&mut s, items, &mut ev);
        self.events.extend(ev);
        self.sigs.push(s);
    }
}

fn emit(s: &mut Sig, items: Vec<crate::demod::Item>, out: &mut Vec<Event>) {
    if items.is_empty() {
        return;
    }
    out.push(Event::Text {
        id: s.id,
        hz: s.hz(),
        text: items.iter().map(|i| i.1).collect(),
        t: items[0].0,
        weak: items.iter().map(|i| i.2 < WEAK).collect(),
    });
}
