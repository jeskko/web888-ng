// SPDX-License-Identifier: GPL-3.0-or-later
//! The passband skimmer: every DGPS beacon in the audio (or IQ) found and
//! decoded.
//!
//! * Every 2 s the last 8 s are searched: the passband mixed to 0 Hz and
//!   squared. MSK squares to two lines a bit rate apart (twice the
//!   carrier ± half the rate); pairs 50, 100 or 200 Hz apart, both
//!   [`LINE_DB`] over the floor, give a beacon's carrier and rate to a
//!   fraction of a hertz. (Carriers, NDBs, square to single lines.)
//! * A candidate gets a channel (±0.75 × the rate: the neighbours are
//!   500 Hz away) at [`SPS`] samples a bit and a [`Demod`], primed from
//!   the last 8 s; it is kept once [`MIN_WORDS`] words in step with a
//!   header pass parity. Others are left alone for 30 s.
//! * A beacon with no word passing parity for 60 s is dropped.

use crate::demod::{Demod, SPS};
use crate::rtcm::Msg;
use num_complex::{Complex32, Complex64};
use rustfft::FftPlanner;
use std::collections::VecDeque;
use web888_cw::dsp;

/// The input audio rate.
pub const AUDIO_RATE: f64 = 12_000.0;
/// The rates tried (Bd).
pub const RATES: [f64; 3] = [50.0, 100.0, 200.0];
/// A squared line this far over the floor (dB).
pub const LINE_DB: f32 = 15.0;
/// Words in step that pass parity before a beacon is listed.
pub const MIN_WORDS: u64 = 3;
/// The widest piece of the band squared at once (Hz).
pub const PIECE_HZ: f64 = 3000.0;
const HISTORY_S: f64 = 8.0;
const DROP_S: f64 = 60.0;

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A beacon found: its carrier (audio Hz, or offset in IQ) and rate.
    Lock { id: u32, hz: f64, baud: f64 },
    /// A message (every word read or not).
    Msg { id: u32, hz: f64, msg: Msg },
}

/// A beacon being decoded.
#[derive(Clone, Debug, PartialEq)]
pub struct Active {
    pub id: u32,
    pub hz: f64,
    pub baud: f64,
    /// Average |phase turn| over 90°: 1 clean.
    pub quality: f32,
    /// Share of words passing parity (recent).
    pub words: f32,
    /// In step with the messages.
    pub synced: bool,
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
    fn new(hz: f64, half_bw: f64, rate: f64, decim: usize, taps: usize) -> Self {
        let n = taps | 1;
        let a = -2.0 * std::f64::consts::PI * hz / rate;
        Channel {
            decim,
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
    off: f64,
    ch: Channel,
    dem: Demod,
    buf: Vec<Complex32>,
    words: f32,
    seen: (u64, u64),
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
    planner: FftPlanner<f32>,
}

impl Default for Skimmer {
    fn default() -> Self {
        Self::new(200.0, 2800.0)
    }
}

impl Skimmer {
    /// Beacons between `lo` and `hi` Hz of 12 kS/s audio.
    pub fn new(lo: f64, hi: f64) -> Self {
        Self::with_rate(AUDIO_RATE, false, lo, hi)
    }

    /// Beacons between `lo` and `hi` Hz (offsets) of complex IQ at `rate`
    /// (a multiple of 2.4 kS/s, e.g. a 60 kS/s bin).
    pub fn new_iq(rate: f64, lo: f64, hi: f64) -> Self {
        let mut s = Self::with_rate(rate, true, lo, hi);
        s.max_signals = 64;
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
            next_run: (HISTORY_S * rate) as u64,
            events: Vec::new(),
            runs: 0,
            every: 2.0,
            max_signals: 16,
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
        self.hi = hi.min(edge).max(self.lo + 200.0);
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
            for msg in s.dem.feed(&s.buf) {
                ev.push(Event::Msg {
                    id: s.id,
                    hz: s.hz(),
                    msg,
                });
            }
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

    /// The beacons being decoded, by frequency.
    pub fn active(&self) -> Vec<Active> {
        let now = self.now();
        let mut v: Vec<Active> = self
            .sigs
            .iter()
            .map(|s| Active {
                id: s.id,
                hz: s.hz(),
                baud: s.dem.baud,
                quality: s.dem.quality,
                words: s.words,
                synced: s.dem.framer.synced() && now - s.dem.framer.last_good < 5.0,
            })
            .collect();
        v.sort_by(|a, b| a.hz.total_cmp(&b.hz));
        v
    }

    fn manage(&mut self) {
        let now = self.now();
        self.ignore.retain(|&(_, u)| u > now);
        for s in &mut self.sigs {
            let f = &s.dem.framer;
            let (g, b) = (f.good - s.seen.0, f.bad - s.seen.1);
            s.seen = (f.good, f.bad);
            if g + b > 0 {
                s.words += (g as f32 / (g + b) as f32 - s.words) * 0.3;
            }
        }
        let mut drop = vec![false; self.sigs.len()];
        for (i, s) in self.sigs.iter().enumerate() {
            if now - s.dem.framer.last_good > DROP_S {
                drop[i] = true;
            }
            for (j, o) in self.sigs.iter().enumerate().take(i) {
                if !drop[j] && (s.hz() - o.hz()).abs() < 0.4 * s.dem.baud.min(o.dem.baud) {
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

    /// Carrier and rate of each MSK signal in the history: pairs of lines
    /// in the squared passband, searched in pieces of at most
    /// [`PIECE_HZ`] (a wide IQ band squared would alias, and a wider
    /// band squared raises the floor: weak beacons are lost).
    pub fn candidates(&mut self) -> Vec<(f64, f64)> {
        let mut out: Vec<(f64, f64, f32)> = Vec::new();
        let width = self.hi - self.lo;
        let pieces = (width / PIECE_HZ).ceil().max(1.0) as usize;
        let w = width / pieces as f64;
        for k in 0..pieces {
            // overlapping by 300 Hz, so a beacon on an edge is whole in one
            let a = (self.lo + k as f64 * w - 150.0).max(self.lo);
            let b = (self.lo + (k + 1) as f64 * w + 150.0).min(self.hi);
            for c in self.piece(a, b) {
                match out.iter_mut().find(|o| (o.0 - c.0).abs() < 0.3 * c.1) {
                    Some(o) if c.2 > o.2 => *o = c,
                    Some(_) => {}
                    None => out.push(c),
                }
            }
        }
        out.sort_by(|a, b| b.2.total_cmp(&a.2));
        out.into_iter().map(|(f, b, _)| (f, b)).collect()
    }

    /// [`Skimmer::candidates`] in lo..hi: (carrier, rate, line level dB).
    fn piece(&mut self, lo: f64, hi: f64) -> Vec<(f64, f64, f32)> {
        let width = hi - lo;
        let centre = 0.5 * (lo + hi);
        // the squared band spans ±width: a rate with room for it
        let decim = ((self.rate / (2.0 * width + 400.0)).floor() as usize).max(1);
        let out = self.rate / decim as f64;
        let taps = ((self.rate / 300.0) * 2.0) as usize;
        let mut ch = Channel::new(centre, 0.5 * width + 50.0, self.rate, decim, taps);
        let mut z = Vec::new();
        ch.push(self.hist.iter().copied(), &mut z);
        let skip = (taps / decim).min(z.len());
        let z = &z[skip..];
        if (z.len() as f64) < 4.0 * out {
            return Vec::new();
        }
        let n = z.len().next_power_of_two();
        let fft = self.planner.plan_fft_forward(n);
        let m = z.len();
        let mut s: Vec<Complex32> = z
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / m as f64).cos();
                v * v * w as f32
            })
            .collect();
        s.resize(n, Complex32::new(0.0, 0.0));
        fft.process(&mut s);
        let bin = out / n as f64;
        let db: Vec<f32> = s
            .iter()
            .map(|v| 10.0 * (v.norm_sqr() + 1e-30).log10())
            .collect();
        // floor: medians of blocks of ~50 Hz
        let blk = ((50.0 / bin) as usize).max(16);
        let floors: Vec<f32> = db.chunks(blk).map(dsp::median).collect();
        let lvl = |k: usize| db[k] - floors[k / blk];
        let freq = |k: usize| {
            let k = k as f64;
            if k < n as f64 / 2.0 {
                k * bin
            } else {
                (k - n as f64) * bin
            }
        };
        // lines: local maxima within ±1 Hz, LINE_DB over the floor
        let w = ((1.0 / bin) as usize).max(1);
        let mut lines: Vec<(f64, f32)> = Vec::new();
        for k in 0..n {
            if lvl(k) < LINE_DB {
                continue;
            }
            if (1..=w).all(|j| db[(k + j) % n] <= db[k] && db[(k + n - j) % n] < db[k]) {
                lines.push((freq(k), lvl(k)));
            }
        }
        lines.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut found: Vec<(f64, f64, f32)> = Vec::new();
        for (i, &(fa, la)) in lines.iter().enumerate() {
            for &(fb, lb) in &lines[i + 1..] {
                let d = fb - fa;
                if d > 210.0 {
                    break;
                }
                let Some(&baud) = RATES.iter().find(|&&r| (d - r).abs() < 0.6) else {
                    continue;
                };
                if (la - lb).abs() > 12.0 {
                    continue;
                }
                let fc = centre + 0.25 * (fa + fb);
                if fc < lo || fc > hi {
                    continue;
                }
                let lv = la.min(lb);
                // one per carrier: the strongest pair
                match found.iter_mut().find(|o| (o.0 - fc).abs() < 0.3 * baud) {
                    Some(o) if lv > o.2 => *o = (fc, baud, lv),
                    Some(_) => {}
                    None => found.push((fc, baud, lv)),
                }
            }
        }
        found
    }

    fn detect(&mut self) {
        if self.sigs.len() >= self.max_signals {
            return;
        }
        let now = self.now();
        for (fc, baud) in self.candidates() {
            if self.sigs.len() >= self.max_signals {
                break;
            }
            if self.ignore.iter().any(|&(h, _)| (h - fc).abs() < 20.0)
                || self
                    .sigs
                    .iter()
                    .any(|s| (s.hz() - fc).abs() < 0.4 * s.dem.baud.max(baud))
            {
                continue;
            }
            if !self.lock(fc, baud) {
                self.ignore.push((fc, now + 30.0));
            }
        }
    }

    /// Decodes it from the history; keeps it if its words pass parity.
    fn lock(&mut self, fc: f64, baud: f64) -> bool {
        let out = baud * SPS as f64;
        let decim = (self.rate / out).round().max(1.0) as usize;
        let taps = ((self.rate / baud) * 4.0) as usize;
        let mut ch = Channel::new(fc, 0.75 * baud, self.rate, decim, taps);
        let n0 = self.n_in - self.hist.len() as u64;
        let t0 = (n0 + decim as u64 - 1) as f64 / self.rate - ch.delay(self.rate);
        let mut dem = Demod::new(baud, t0);
        let mut buf = Vec::new();
        ch.push(self.hist.iter().copied(), &mut buf);
        let msgs = dem.feed(&buf);
        if dem.framer.good < MIN_WORDS {
            return false;
        }
        let id = self.next_id;
        self.next_id += 1;
        let f = &dem.framer;
        let words = f.good as f32 / (f.good + f.bad).max(1) as f32;
        let seen = (f.good, f.bad);
        self.events.push(Event::Lock { id, hz: fc, baud });
        let s = Sig {
            id,
            off: fc,
            ch,
            dem,
            buf: Vec::new(),
            words,
            seen,
        };
        for msg in msgs {
            self.events.push(Event::Msg {
                id,
                hz: s.hz(),
                msg,
            });
        }
        self.sigs.push(s);
        true
    }
}
