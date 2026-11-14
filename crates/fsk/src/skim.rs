// SPDX-License-Identifier: GPL-3.0-or-later
//! The passband skimmer: every two-tone FSK signal in the audio is found,
//! analysed and, when that yields believable text, locked and decoded as
//! a stream. After datarx's `wahrx/fskskim.py`, on real audio.
//!
//! * A 768-point spectrogram (15.6 Hz lines, 32 ms hop) of the band; every
//!   2 s the last 8 s are searched for tone pairs whose energies alternate.
//! * A new pair gets a 4 kS/s complex channel centred between its tones
//!   (primed from the last 8 s of audio) and is analysed once 4 s are in.
//! * A believable analysis (framing errors ≤ 12 %, plausible text) locks
//!   it: a streaming decoder then runs with those parameters. Pairs that
//!   never decode (FT8/FT4 tones, unrelated carriers) are dropped and their
//!   frequency left alone for 2 minutes; signals identified but not
//!   decodable (reversals, 4-of-7 codes) are listed as such for 15 minutes.
//!   A locked signal whose framing errors pile up is analysed again.

use crate::analyze::{self, FskDecoder, FskParams, Item, Mode};
use num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use std::collections::VecDeque;
use std::sync::Arc;
use web888_cw::dsp;

/// The input audio rate.
pub const AUDIO_RATE: f64 = 12_000.0;
/// The channels' rate.
pub const CH_FS: f64 = 4000.0;
const DECIM: usize = 3;
const NF: usize = 768;
const HOP: usize = 384;
const HISTORY_S: f64 = 8.0;
const FRAMES_S: f64 = 8.0;
const WINDOW_S: f64 = 12.0;

/// What the skimmer reports.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A signal's parameters were worked out; text follows.
    Lock { id: u32, hz: f64, params: FskParams },
    /// New text of a locked signal (`t`: stream time of its first char).
    Text {
        id: u32,
        hz: f64,
        text: String,
        t: f64,
    },
    /// An FSK signal identified but not decodable.
    Ident { hz: f64, params: FskParams },
}

/// A signal as listed: locked (decoding) or identified.
#[derive(Clone, Debug, PartialEq)]
pub struct Active {
    /// 0 for an identified one.
    pub id: u32,
    /// Audio frequency of the centre between the tones.
    pub hz: f64,
    pub params: FskParams,
    pub snr: f64,
    pub fe: f64,
    pub locked: bool,
}

/// One signal's channel: audio mixed from `hz` and decimated to CH_FS.
struct Channel {
    taps: Vec<f32>,
    ring: Vec<Complex32>,
    pos: usize,
    phase: f64,
    step: f64,
    count: usize,
}

impl Channel {
    fn new(hz: f64, half_bw: f64) -> Self {
        let n = 97;
        Channel {
            taps: dsp::firwin(n, half_bw, AUDIO_RATE),
            ring: vec![Complex32::new(0.0, 0.0); 2 * n],
            pos: 0,
            phase: 0.0,
            step: -2.0 * std::f64::consts::PI * hz / AUDIO_RATE,
            count: 0,
        }
    }

    fn delay(&self) -> f64 {
        (self.taps.len() - 1) as f64 / 2.0 / AUDIO_RATE
    }

    fn push(&mut self, audio: &[f32], out: &mut Vec<Complex32>) {
        let n = self.taps.len();
        for &a in audio {
            let v = Complex32::new(
                (a as f64 * self.phase.cos()) as f32,
                (a as f64 * self.phase.sin()) as f32,
            );
            self.phase += self.step;
            if self.phase < -std::f64::consts::PI {
                self.phase += 2.0 * std::f64::consts::PI;
            }
            self.ring[self.pos] = v;
            self.ring[self.pos + n] = v;
            self.pos = (self.pos + 1) % n;
            self.count += 1;
            if self.count == DECIM {
                self.count = 0;
                let mut acc = Complex32::new(0.0, 0.0);
                for (x, h) in self.ring[self.pos..self.pos + n].iter().zip(&self.taps) {
                    acc += x * *h;
                }
                out.push(acc * 2.0);
            }
        }
    }
}

struct Sig {
    id: u32,
    /// Channel centre (audio Hz).
    off: f64,
    /// Tones relative to the centre.
    lo: f64,
    hi: f64,
    born: f64,
    ch: Channel,
    buf: Vec<Complex32>,
    buf_t: f64,
    dec: Option<FskDecoder>,
    tries: u32,
    next_try: f64,
    last_text: f64,
    ident: bool,
}

impl Sig {
    fn hz(&self) -> f64 {
        match &self.dec {
            Some(d) => self.off + 0.5 * (d.p.f_lo + d.p.f_hi),
            None => self.off + 0.5 * (self.lo + self.hi),
        }
    }
}

struct Ident {
    hz: f64,
    params: FskParams,
    seen: f64,
}

pub struct Skimmer {
    lo: f64,
    hi: f64,
    n_in: u64,
    hist: VecDeque<f32>,
    pend: Vec<f32>,
    frames: VecDeque<Vec<f32>>,
    bins: (usize, usize),
    fft: Arc<dyn Fft<f32>>,
    win: Vec<f32>,
    sigs: Vec<Sig>,
    ignore: Vec<(f64, f64)>,
    idents: Vec<Ident>,
    next_id: u32,
    next_run: u64,
    events: Vec<Event>,
    pub runs: u64,
    pub every: f64,
    pub max_signals: usize,
    pub min_db: f32,
}

impl Default for Skimmer {
    fn default() -> Self {
        Self::new(200.0, 2800.0)
    }
}

fn rank(p: &FskParams) -> u8 {
    if p.charset.starts_with("4-of-7") {
        2
    } else if p.charset.starts_with("reversals") {
        1
    } else {
        0
    }
}

impl Skimmer {
    pub fn new(lo: f64, hi: f64) -> Self {
        let mut s = Skimmer {
            lo,
            hi,
            n_in: 0,
            hist: VecDeque::new(),
            pend: Vec::new(),
            frames: VecDeque::new(),
            bins: (0, 0),
            fft: FftPlanner::new().plan_fft_forward(NF),
            win: (0..NF)
                .map(|i| {
                    (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / NF as f64).cos()) as f32
                })
                .collect(),
            sigs: Vec::new(),
            ignore: Vec::new(),
            idents: Vec::new(),
            next_id: 1,
            next_run: (2.0 * AUDIO_RATE) as u64,
            events: Vec::new(),
            runs: 0,
            every: 2.0,
            max_signals: 8,
            min_db: 10.0,
        };
        s.set_band(lo, hi);
        s
    }

    /// The audio band searched. Signals outside it are dropped.
    pub fn set_band(&mut self, lo: f64, hi: f64) {
        let df = AUDIO_RATE / NF as f64;
        self.lo = lo.max(50.0);
        self.hi = hi.min(AUDIO_RATE / 2.0 - 100.0).max(self.lo + 8.0 * df);
        self.bins = (
            (self.lo / df).ceil() as usize,
            (self.hi / df).floor() as usize + 1,
        );
        self.frames.clear();
        let (a, b) = (self.lo, self.hi);
        self.sigs.retain(|s| (a..=b).contains(&s.hz()));
    }

    pub fn now(&self) -> f64 {
        self.n_in as f64 / AUDIO_RATE
    }

    pub fn push(&mut self, audio: &[f32]) {
        let keep = (WINDOW_S * CH_FS) as usize;
        let mut new_events = Vec::new();
        for s in &mut self.sigs {
            let n0 = s.buf.len();
            s.ch.push(audio, &mut s.buf);
            if let Some(dec) = &mut s.dec {
                let items = dec.feed(&s.buf[n0..]);
                emit(s, items, &mut new_events);
            }
            if s.buf.len() > keep {
                let cut = s.buf.len() - keep;
                s.buf.drain(..cut);
                s.buf_t += cut as f64 / CH_FS;
            }
        }
        self.events.extend(new_events);
        self.hist.extend(audio);
        let hk = (HISTORY_S * AUDIO_RATE) as usize;
        if self.hist.len() > hk {
            let cut = self.hist.len() - hk;
            self.hist.drain(..cut);
        }
        self.pend.extend_from_slice(audio);
        let mut i = 0;
        while i + NF <= self.pend.len() {
            let mut b: Vec<Complex32> = (0..NF)
                .map(|k| Complex32::new(self.pend[i + k] * self.win[k], 0.0))
                .collect();
            self.fft.process(&mut b);
            self.frames.push_back(
                b[self.bins.0..self.bins.1]
                    .iter()
                    .map(|v| v.norm_sqr())
                    .collect(),
            );
            i += HOP;
        }
        self.pend.drain(..i);
        let max = (FRAMES_S * AUDIO_RATE / HOP as f64) as usize;
        while self.frames.len() > max {
            self.frames.pop_front();
        }
        self.n_in += audio.len() as u64;
        if self.n_in >= self.next_run {
            self.next_run += (self.every * AUDIO_RATE) as u64;
            self.runs += 1;
            self.detect();
            self.manage();
        }
    }

    fn detect(&mut self) {
        let nt = self.frames.len();
        let nb = self.bins.1 - self.bins.0;
        if self.sigs.len() >= self.max_signals
            || (nt as f64) < 3.0 * AUDIO_RATE / HOP as f64
            || nb < 8
        {
            return;
        }
        let df = AUDIO_RATE / NF as f64;
        let f: Vec<f64> = (self.bins.0..self.bins.1).map(|k| k as f64 * df).collect();
        let s: Vec<Vec<f32>> = self.frames.iter().cloned().collect();
        let mut mean = vec![0f64; nb];
        for row in &s {
            for (m, v) in mean.iter_mut().zip(row) {
                *m += *v as f64;
            }
        }
        let mean_db: Vec<f32> = mean
            .iter()
            .map(|m| (10.0 * (m / nt as f64 + 1e-20).log10()) as f32)
            .collect();
        let floor = dsp::percentile_filter(&mean_db, 50.0, ((3000.0 / df) as usize | 1).max(3));
        let (fp, lv, p) = analyze::find_peaks(&s, &f, self.min_db, Some(&floor));
        let now = self.now();
        self.ignore.retain(|&(_, u)| u > now);
        for (lo, hi, _, _) in analyze::pair_peaks(&fp, &lv, &p, 60.0, 1100.0, 14.0) {
            if self.sigs.len() >= self.max_signals {
                break;
            }
            let c = 0.5 * (lo + hi);
            if self.ignore.iter().any(|&(f, _)| (c - f).abs() < 60.0)
                || self
                    .sigs
                    .iter()
                    .any(|s| (c - s.hz()).abs() < 0.5 * (hi - lo).max(100.0))
            {
                continue;
            }
            let mut ch = Channel::new(c, 0.5 * (hi - lo) + 250.0);
            let mut buf = Vec::new();
            let hist: Vec<f32> = self.hist.iter().copied().collect();
            ch.push(&hist, &mut buf);
            let n0 = self.n_in - hist.len() as u64;
            let buf_t = (n0 + DECIM as u64 - 1) as f64 / AUDIO_RATE - ch.delay();
            self.sigs.push(Sig {
                id: self.next_id,
                off: c,
                lo: lo - c,
                hi: hi - c,
                born: now,
                ch,
                buf,
                buf_t,
                dec: None,
                tries: 0,
                next_try: now,
                last_text: now,
                ident: false,
            });
            self.next_id += 1;
        }
    }

    fn manage(&mut self) {
        let now = self.now();
        let mut i = 0;
        while i < self.sigs.len() {
            let s = &mut self.sigs[i];
            if s.dec.is_none() {
                if now >= s.next_try && s.buf.len() as f64 >= 4.0 * CH_FS {
                    s.tries += 1;
                    s.next_try = now + 3.0;
                    let r = analyze::analyze(&s.buf, CH_FS, Some((s.lo, s.hi)), s.buf_t);
                    if analyze::is_valid(&r, 12)
                        && r.params.as_ref().is_some_and(|p| p.charset != "raw")
                    {
                        let p = r.params.expect("valid");
                        let mut dec = FskDecoder::new(CH_FS, p.clone(), s.buf_t);
                        let id = s.id;
                        let hz = s.off + 0.5 * (p.f_lo + p.f_hi);
                        self.events.push(Event::Lock { id, hz, params: p });
                        let items = dec.feed(&s.buf);
                        s.dec = Some(dec);
                        s.last_text = now;
                        let mut ev = Vec::new();
                        emit(s, items, &mut ev);
                        self.events.extend(ev);
                        i += 1;
                        continue;
                    }
                    if let Some(q) = &r.params {
                        s.lo = q.f_lo;
                        s.hi = q.f_hi;
                        // identified but not decodable: only clear two-tone signals
                        if q.mode == Mode::Sync
                            && q.snr >= -3.0
                            && (q.charset != "framing unknown"
                                || (q.snr >= 0.0 && r.baud_fit >= 0.6))
                        {
                            s.ident = q.charset != "framing unknown";
                            let hz = s.off + 0.5 * (q.f_lo + q.f_hi);
                            let q = q.clone();
                            self.note_ident(hz, q);
                        }
                    }
                    let s = &self.sigs[i];
                    if s.ident {
                        self.drop(i, 600.0);
                        continue;
                    }
                    if s.tries >= 3 || (r.params.is_none() && s.buf.len() as f64 >= 6.0 * CH_FS) {
                        self.drop(i, 120.0);
                        continue;
                    }
                }
                let s = &self.sigs[i];
                if now - s.born > 24.0 {
                    let ig = if s.ident { 600.0 } else { 120.0 };
                    self.drop(i, ig);
                    continue;
                }
                i += 1;
                continue;
            }
            let silent = now - s.last_text.max(s.born);
            let dec = s.dec.as_ref().expect("locked");
            if silent > 40.0 {
                self.drop(i, 0.0);
                continue;
            }
            if dec.n_chars >= 30 && dec.fe_rate() > 0.3 {
                // another station or other settings: analyse afresh
                s.dec = None;
                s.tries = 0;
                s.next_try = now + 4.0;
                s.born = now;
                let k = (2.0 * CH_FS) as usize;
                if s.buf.len() > k {
                    let cut = s.buf.len() - k;
                    s.buf.drain(..cut);
                    s.buf_t += cut as f64 / CH_FS;
                }
            }
            i += 1;
        }
        self.idents.retain(|d| now - d.seen < 900.0);
    }

    fn note_ident(&mut self, hz: f64, p: FskParams) {
        let now = self.now();
        if let Some(old) = self.idents.iter_mut().find(|d| (d.hz - hz).abs() < 75.0) {
            if rank(&old.params) >= rank(&p) && (old.params.baud - p.baud).abs() <= 1.0 {
                old.seen = now;
                return;
            }
            old.hz = hz;
            old.params = p.clone();
            old.seen = now;
        } else {
            self.idents.push(Ident {
                hz,
                params: p.clone(),
                seen: now,
            });
        }
        self.events.push(Event::Ident { hz, params: p });
    }

    fn drop(&mut self, i: usize, ignore: f64) {
        let s = self.sigs.remove(i);
        if ignore > 0.0 {
            self.ignore.push((s.hz(), self.now() + ignore));
        }
    }

    pub fn poll(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// Locked signals and identified ones, by frequency.
    pub fn active(&self) -> Vec<Active> {
        let mut out: Vec<Active> = self
            .sigs
            .iter()
            .filter_map(|s| {
                s.dec.as_ref().map(|d| Active {
                    id: s.id,
                    hz: s.hz(),
                    params: d.p.clone(),
                    snr: d.snr(),
                    fe: d.fe_rate(),
                    locked: true,
                })
            })
            .collect();
        for d in &self.idents {
            if !out.iter().any(|a| (a.hz - d.hz).abs() < 100.0) {
                out.push(Active {
                    id: 0,
                    hz: d.hz,
                    params: d.params.clone(),
                    snr: d.params.snr,
                    fe: 0.0,
                    locked: false,
                });
            }
        }
        out.sort_by(|a, b| a.hz.total_cmp(&b.hz));
        out
    }
}

fn emit(s: &mut Sig, items: Vec<Item>, out: &mut Vec<Event>) {
    if items.is_empty() {
        return;
    }
    let text: String = items.iter().map(|i| i.1.as_str()).collect();
    s.last_text = items[items.len() - 1].0;
    out.push(Event::Text {
        id: s.id,
        hz: s.hz(),
        text,
        t: items[0].0,
    });
}
