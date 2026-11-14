// SPDX-License-Identifier: GPL-3.0-or-later
//! The passband skimmer: every keyed carrier in a stretch of audio gets its
//! own channel and decoder, and its text streams out as it settles. After
//! datarx's `wahrx/cwskim.py`, on real audio instead of a wideband IQ bank.
//!
//! * Detection: a 512-point spectrogram (23.4 Hz lines, 21 ms hop; short enough for 40 WPM key-up gaps) of the band, the
//!   last 20 s; a keyed carrier is a line whose 90th percentile level is
//!   15 dB over the band's local floor, whose 20th percentile is back near
//!   it, and which is on 3-90 % of the time.
//! * A new signal's channel is run over the last 8 s of audio first, so a
//!   call found a few seconds late still decodes from its start.
//! * Every 2 s each signal's last 30 s are decoded again; characters are
//!   emitted once they are 3 s (or 40 dots) old, so the end of the window
//!   (decided with too little context) is never shown.

use crate::channel::{AUDIO_RATE, Channel};
use crate::decode::{self, DecodeOpts, ENV_FS};
use crate::dsp;
use num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use std::collections::VecDeque;
use std::sync::Arc;

const NF: usize = 512;
const HOP: usize = 256;
const HISTORY_S: f64 = 8.0;
const FRAMES_S: f64 = 20.0;

/// New text of one signal.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub id: u32,
    /// Audio frequency (Hz).
    pub hz: f64,
    pub text: String,
    pub wpm: f64,
    pub snr: f64,
    /// Stream time of the text's first character (s since the reset).
    pub t: f64,
}

/// A signal being decoded.
#[derive(Clone, Debug, PartialEq)]
pub struct Active {
    pub id: u32,
    pub hz: f64,
    pub wpm: f64,
    pub snr: f64,
}

struct Sig {
    id: u32,
    off: f64,
    ch: Channel,
    buf: Vec<Complex32>,
    buf_t: f64,
    born: f64,
    wpm: Option<f64>,
    snr: f64,
    freq_fine: f64,
    last_t: f64,
    last_mark: f64,
    good: bool,
    text: String,
}

impl Sig {
    fn hz(&self) -> f64 {
        self.off + self.freq_fine
    }
}

pub struct Skimmer {
    lo: f64,
    hi: f64,
    /// Samples in since the reset.
    n_in: u64,
    hist: VecDeque<f32>,
    pend: Vec<f32>,
    frames: VecDeque<Vec<f32>>,
    bins: (usize, usize),
    fft: Arc<dyn Fft<f32>>,
    win: Vec<f32>,
    sigs: Vec<Sig>,
    next_id: u32,
    next_run: u64,
    events: Vec<Event>,
    /// Decode runs so far (each changes [`Skimmer::active`]).
    pub runs: u64,
    /// Seconds of each signal decoded every time.
    pub window: f64,
    /// Seconds between decodes.
    pub every: f64,
    pub max_signals: usize,
    /// Detection level over the floor (dB).
    pub min_db: f32,
}

impl Default for Skimmer {
    fn default() -> Self {
        Self::new(200.0, 2800.0)
    }
}

impl Skimmer {
    /// Signals between `lo` and `hi` Hz of audio.
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
            next_id: 1,
            next_run: (2.0 * AUDIO_RATE) as u64,
            events: Vec::new(),
            runs: 0,
            window: 30.0,
            every: 2.0,
            max_signals: 12,
            min_db: 15.0,
        };
        s.set_band(lo, hi);
        s
    }

    /// The audio band searched (the passband, inside its skirts). Signals
    /// outside it are dropped; the spectrogram starts over.
    pub fn set_band(&mut self, lo: f64, hi: f64) {
        let (lo, hi) = (lo.max(50.0), hi.min(AUDIO_RATE / 2.0 - 100.0));
        self.lo = lo;
        self.hi = hi.max(lo + 3.0 * AUDIO_RATE / NF as f64);
        let df = AUDIO_RATE / NF as f64;
        self.bins = (
            (self.lo / df).ceil() as usize,
            (self.hi / df).floor() as usize + 1,
        );
        self.frames.clear();
        let (a, b) = (self.lo - 20.0, self.hi + 20.0);
        self.sigs.retain(|s| (a..=b).contains(&s.hz()));
    }

    /// Stream time (s) of the newest channel sample.
    pub fn now(&self) -> f64 {
        self.n_in as f64 / AUDIO_RATE - Channel::delay()
    }

    pub fn push(&mut self, audio: &[f32]) {
        for s in &mut self.sigs {
            s.ch.push(audio, &mut s.buf);
            let keep = (self.window * ENV_FS) as usize;
            if s.buf.len() > keep {
                let cut = s.buf.len() - keep;
                s.buf.drain(..cut);
                s.buf_t += cut as f64 / ENV_FS;
            }
        }
        self.hist.extend(audio);
        let keep = (HISTORY_S * AUDIO_RATE) as usize;
        if self.hist.len() > keep {
            let cut = self.hist.len() - keep;
            self.hist.drain(..cut);
        }
        self.pend.extend_from_slice(audio);
        let mut i = 0;
        while i + NF <= self.pend.len() {
            self.frame(i);
            i += HOP;
        }
        self.pend.drain(..i);
        self.n_in += audio.len() as u64;
        if self.n_in >= self.next_run {
            self.next_run += (self.every * AUDIO_RATE) as u64;
            self.detect();
            self.decode_all();
        }
    }

    fn frame(&mut self, at: usize) {
        let mut b: Vec<Complex32> = (0..NF)
            .map(|k| Complex32::new(self.pend[at + k] * self.win[k], 0.0))
            .collect();
        self.fft.process(&mut b);
        self.frames.push_back(
            b[self.bins.0..self.bins.1]
                .iter()
                .map(|v| v.norm_sqr())
                .collect(),
        );
        let max = (FRAMES_S * AUDIO_RATE / HOP as f64) as usize;
        while self.frames.len() > max {
            self.frames.pop_front();
        }
    }

    /// Keyed carriers: (audio Hz, level dB over the floor), strongest first.
    fn candidates(&self) -> Vec<(f64, f32)> {
        let nt = self.frames.len();
        let nb = self.bins.1 - self.bins.0;
        if nt < 40 || nb < 3 {
            return Vec::new();
        }
        let df = AUDIO_RATE / NF as f64;
        let (mut hi, mut lo, mut med) = (vec![0f32; nb], vec![0f32; nb], vec![0f32; nb]);
        let mut cols: Vec<Vec<f32>> = vec![Vec::with_capacity(nt); nb];
        for t in 0..nt {
            let (a, b, c) = (
                &self.frames[t.saturating_sub(1)],
                &self.frames[t],
                &self.frames[(t + 1).min(nt - 1)],
            );
            for k in 0..nb {
                cols[k].push(10.0 * ((a[k] + b[k] + c[k]) / 3.0 + 1e-20).log10());
            }
        }
        for k in 0..nb {
            let mut s = cols[k].clone();
            s.sort_by(|a, b| a.total_cmp(b));
            med[k] = dsp::percentile_sorted(&s, 50.0);
            hi[k] = dsp::percentile_sorted(&s, 90.0);
            lo[k] = dsp::percentile_sorted(&s, 20.0);
        }
        let floor = dsp::percentile_filter(&med, 50.0, ((2000.0 / df) as usize | 1).max(3));
        for k in 0..nb {
            hi[k] -= floor[k];
            lo[k] -= floor[k];
        }
        let duty: Vec<f32> = (0..nb)
            .map(|k| {
                cols[k]
                    .iter()
                    .filter(|&&v| v - floor[k] > 0.5 * hi[k])
                    .count() as f32
                    / nt as f32
            })
            .collect();
        let w = ((30.0 / df).round() as usize).max(1);
        let mut out = Vec::new();
        for k in 0..nb {
            let peak = (k.saturating_sub(w)..=(k + w).min(nb - 1)).all(|j| hi[k] >= hi[j]);
            if peak && hi[k] >= self.min_db && lo[k] <= 6.0 && (0.03..=0.9).contains(&duty[k]) {
                let (a, b, c) = (hi[k.saturating_sub(1)], hi[k], hi[(k + 1).min(nb - 1)]);
                let den = a - 2.0 * b + c;
                let d = if den < 0.0 { 0.5 * (a - c) / den } else { 0.0 };
                out.push(((self.bins.0 + k) as f64 * df + d as f64 * df, hi[k]));
            }
        }
        out.sort_by(|a, b| b.1.total_cmp(&a.1));
        out
    }

    fn detect(&mut self) {
        for (off, _) in self.candidates() {
            if self.sigs.len() >= self.max_signals {
                break;
            }
            if self
                .sigs
                .iter()
                .any(|s| (off - s.off).abs().min((off - s.hz()).abs()) < 70.0)
            {
                continue;
            }
            let mut ch = Channel::new(off);
            let mut buf = Vec::new();
            let hist: Vec<f32> = self.hist.iter().copied().collect();
            ch.push(&hist, &mut buf);
            let n0 = self.n_in - hist.len() as u64;
            let decim = (AUDIO_RATE / ENV_FS) as u64;
            let buf_t = (n0 + decim - 1) as f64 / AUDIO_RATE - Channel::delay();
            let now = self.now();
            self.sigs.push(Sig {
                id: self.next_id,
                off,
                ch,
                buf,
                buf_t,
                born: now,
                wpm: None,
                snr: -99.0,
                freq_fine: 0.0,
                last_t: -1.0,
                last_mark: 0.0,
                good: false,
                text: String::new(),
            });
            self.next_id += 1;
        }
    }

    fn decode_all(&mut self) {
        self.runs += 1;
        let now = self.now();
        let mut drop = Vec::new();
        for i in 0..self.sigs.len() {
            let s = &mut self.sigs[i];
            if s.buf.len() < (3.0 * ENV_FS) as usize {
                continue;
            }
            let r = decode::decode(
                &s.buf,
                &DecodeOpts {
                    f0: Some(0.0),
                    span: 40.0,
                    wpm: s.wpm,
                    t0: s.buf_t,
                    split: true,
                    strict: true,
                },
            );
            if r.wpm > 0.0
                && r.marks >= 8
                && (5.0..=60.0).contains(&r.wpm)
                && r.snr > -15.0
                && r.cost < 0.05
            {
                // follow a good fit slowly (an operator drifts; a new one is caught by the search)
                s.wpm = Some(match s.wpm {
                    Some(w) => (0.7 * w + 0.3 * r.wpm).clamp(0.9 * w, 1.1 * w),
                    None => r.wpm,
                });
                s.snr = r.snr;
                s.freq_fine = r.freq;
                s.good = true;
            }
            let u = 1.2 / s.wpm.unwrap_or(20.0);
            let hold = (40.0 * u).max(3.0);
            // re-decodes move start times by a few ms: closer than 2 units to
            // the last emitted character is that one again
            let new: Vec<(f64, String)> = if s.good {
                r.chars
                    .into_iter()
                    .filter(|(t, _)| s.last_t + 2.0 * u < *t && *t < now - hold)
                    .collect()
            } else {
                Vec::new()
            };
            if let Some(last) = new.last() {
                s.last_t = last.0;
                if let Some(m) = new.iter().rev().find(|c| c.1 != " ") {
                    s.last_mark = s.last_mark.max(m.0);
                }
                let mut txt: String = new.iter().map(|c| c.1.as_str()).collect();
                if s.text.ends_with(' ') && txt.starts_with(' ') {
                    txt = txt.trim_start_matches(' ').to_string();
                }
                if !txt.trim().is_empty() || (!s.text.is_empty() && !s.text.ends_with(' ')) {
                    s.text.push_str(&txt);
                    if s.text.len() > 400 {
                        let cut = s.text.len() - 400;
                        let cut = (cut..s.text.len())
                            .find(|&c| s.text.is_char_boundary(c))
                            .unwrap_or(0);
                        s.text.drain(..cut);
                    }
                    self.events.push(Event {
                        id: s.id,
                        hz: s.hz(),
                        text: txt,
                        wpm: s.wpm.unwrap_or(0.0),
                        snr: s.snr,
                        t: new[0].0,
                    });
                }
            }
            let idle = now - s.last_mark.max(s.born);
            if idle > 60.0 || (!s.good && now - s.born > 20.0) || self.shadow(i) {
                drop.push(i);
            }
        }
        for i in drop.into_iter().rev() {
            self.sigs.remove(i);
        }
    }

    /// A weaker signal within 150 Hz that decodes the same text (a key
    /// click or sideband of a strong one).
    fn shadow(&self, i: usize) -> bool {
        let s = &self.sigs[i];
        let a: String = s.text.chars().filter(|&c| c != ' ').collect();
        let a: Vec<char> = a.chars().collect();
        if a.len() < 12 {
            return false;
        }
        let tail: String = a[a.len() - 12..].iter().collect();
        self.sigs.iter().enumerate().any(|(j, o)| {
            j != i
                && (o.hz() - s.hz()).abs() <= 150.0
                && o.snr >= s.snr
                && o.text
                    .chars()
                    .filter(|&c| c != ' ')
                    .collect::<String>()
                    .contains(&tail)
        })
    }

    /// New text since the last call.
    pub fn poll(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// Signals that have decoded, by frequency.
    pub fn active(&self) -> Vec<Active> {
        let mut v: Vec<Active> = self
            .sigs
            .iter()
            .filter(|s| s.good)
            .map(|s| Active {
                id: s.id,
                hz: s.hz(),
                wpm: s.wpm.unwrap_or(0.0),
                snr: s.snr,
            })
            .collect();
        v.sort_by(|a, b| a.hz.total_cmp(&b.hz));
        v
    }
}
