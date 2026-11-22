// SPDX-License-Identifier: GPL-3.0-or-later
//! RBU, 66 2/3 kHz from Taldom near Moscow (VNIIFTRI's format, as the
//! RBU article in Wikipedia gives it from the official specification and
//! the ITU-R TF.768 supplement).
//!
//! Every 100 ms, on the UTC tenth of a second, one bit: 10 ms of plain
//! carrier, 80 ms of carrier phase-modulated (index 0.698) by a 100 Hz
//! tone (0) or a 312.5 Hz tone (1), 5 ms plain, 5 ms carrier off. Of a
//! second's ten bits the first two carry the time code, the 900 ms one is
//! always 1 (the second's marker), 200-600 ms always 0, and 700-800 ms 1
//! only in the second before a minute: with the code's first two bits
//! (always 1) the minute starts after five 1 bits in a row. The code is
//! the Moscow time of the following minute, with its offset from UTC.
//!
//! The receiver: the carrier (a tone in the audio) mixed to 0 Hz and
//! decimated to 2 kS/s; the tenth's phase from the 5 ms gaps folded over
//! 20 s; each bit's phase deviation against its slot's mean carrier,
//! measured at 100 and 312.5 Hz; the second's phase from where the
//! always-1 bit is and the always-0 ones aren't; the minute from its
//! marker.

use crate::{Event, Minute, Second, Station, civil_from_days, days_from_civil};
use num_complex::{Complex32, Complex64};
use std::collections::VecDeque;
use std::f64::consts::PI;

/// Decimated rate.
const FS2: f64 = 2000.0;
/// Samples per 100 ms slot at FS2.
const SLOT: usize = 200;
/// Kept: 40 s.
const KEEP: usize = 40 * 2000;

/// `slot_bit`'s window (Hann over its 152 samples) and, per sample, the
/// unit phasors at -100 and -312.5 Hz.
fn slot_table() -> &'static [(f64, [Complex64; 2])] {
    static T: std::sync::OnceLock<Vec<(f64, [Complex64; 2])>> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let n = 176 - 24;
        (0..n)
            .map(|i| {
                let w = 0.5 - 0.5 * (2.0 * PI * i as f64 / (n - 1) as f64).cos();
                let t = i as f64 / FS2;
                let r = |f: f64| {
                    let th = -2.0 * PI * f * t;
                    Complex64::new(th.cos(), th.sin())
                };
                (w, [r(100.0), r(312.5)])
            })
            .collect()
    })
}

pub struct Rbu {
    fs: f64,
    t0: f64,
    dec: usize,
    /// Mixer.
    /// Mixer: e^{-jφ} as a rotator, and its step per sample.
    lo: Complex64,
    lo_step: Complex64,
    /// Boxcar over `dec * 2` input samples (1 ms at 12 kS/s), decimated.
    box_: VecDeque<Complex64>,
    box_sum: Complex64,
    n_in: u64,
    /// Decimated samples, z[base..].
    z: Vec<Complex32>,
    base: u64,
    /// The slot phase (index of a slot start mod SLOT) and when it was set.
    slot_phase: Option<usize>,
    phase_at: u64,
    /// The next slot to read (absolute decimated index of its start).
    next_slot: Option<u64>,
    /// Slots read: (start index, bit, quality).
    slots: VecDeque<(u64, u8, f32)>,
    /// Which slot of ten is the 900 ms one (slot number mod 10).
    marker: Option<usize>,
    /// The seconds read since the minute started (or the last 60).
    secs: Vec<(Second, u8, u8)>,
    minute_at: Option<usize>,
    passed: Vec<(f64, i64)>,
    out: Vec<Event>,
    pub level: f32,
}

impl Rbu {
    /// `tone`: the carrier's audio frequency (Hz); `t0`: the stream time
    /// of the first sample.
    pub fn new(fs: f64, tone: f64, t0: f64) -> Self {
        let dec = (fs / FS2).round().max(1.0) as usize;
        Rbu {
            fs,
            t0,
            dec,
            lo: Complex64::new(1.0, 0.0),
            lo_step: Complex64::from_polar(1.0, -2.0 * PI * tone / fs),
            box_: VecDeque::new(),
            box_sum: Complex64::new(0.0, 0.0),
            n_in: 0,
            z: Vec::new(),
            base: 0,
            slot_phase: None,
            phase_at: 0,
            next_slot: None,
            slots: VecDeque::new(),
            marker: None,
            secs: Vec::new(),
            minute_at: None,
            passed: Vec::new(),
            out: Vec::new(),
            level: 0.0,
        }
    }

    pub fn set_tone(&mut self, tone: f64) {
        self.lo_step = Complex64::from_polar(1.0, -2.0 * PI * tone / self.fs);
    }

    /// Stream time of decimated sample k: the boxcar's centre.
    fn t_of(&self, k: u64) -> f64 {
        let w = 2 * self.dec;
        self.t0 + ((k as f64 + 1.0) * self.dec as f64 - (w as f64 - 1.0) / 2.0) / self.fs
    }

    pub fn push(&mut self, x: &[f32]) {
        let w = 2 * self.dec;
        for &v in x {
            let z = self.lo * v as f64;
            self.lo *= self.lo_step;
            self.box_.push_back(z);
            self.box_sum += z;
            if self.box_.len() > w {
                self.box_sum -= self.box_.pop_front().unwrap_or_default();
            }
            self.n_in += 1;
            if self.n_in.is_multiple_of(self.dec as u64) && self.box_.len() == w {
                let s = self.box_sum / w as f64;
                self.z.push(Complex32::new(s.re as f32, s.im as f32));
            }
            if self.n_in.is_multiple_of(self.fs as u64 * 10) {
                self.box_sum = self.box_.iter().sum();
            }
        }
        self.lo /= self.lo.norm();
        self.read();
        if self.z.len() > KEEP + 2 * FS2 as usize {
            let d = self.z.len() - KEEP;
            self.z.drain(..d);
            self.base += d as u64;
        }
    }

    pub fn poll(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.out)
    }

    /// The tenth's phase: where the 5 ms gaps are, folded over 20 s; the
    /// slot starts where the carrier comes back. Also the level: the
    /// carrier against the gaps.
    fn find_slot_phase(&mut self) -> Option<usize> {
        let n = (20 * FS2 as usize).min(self.z.len()) / SLOT * SLOT;
        if n < 5 * FS2 as usize {
            return None;
        }
        let start = self.z.len() - n;
        let first = self.base + start as u64;
        let mut fold = [0f64; SLOT];
        for (i, s) in self.z[start..].iter().enumerate() {
            fold[((first + i as u64) % SLOT as u64) as usize] += s.norm() as f64;
        }
        // the 5 ms (10 samples) of least carrier
        let gap = 10;
        let (m, low) = (0..SLOT)
            .map(|k| {
                (
                    k,
                    (0..gap).map(|j| fold[(k + j) % SLOT]).sum::<f64>() / gap as f64,
                )
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))?;
        let mean = fold.iter().sum::<f64>() / SLOT as f64;
        self.level = (20.0 * (mean / low.max(1e-30)).log10()) as f32;
        (mean > 2.0 * low).then_some((m + gap) % SLOT)
    }

    /// One slot's bit from z[s..s+SLOT]: the phase deviation against the
    /// slot's mean carrier, its power at 100 and 312.5 Hz (Hann window
    /// over 12-88 ms). Quality: their difference over their sum.
    fn slot_bit(z: &[Complex32]) -> (u8, f32) {
        let tab = slot_table();
        let (a, b) = (24, 176);
        let c: Complex32 = z[a..b].iter().sum();
        let mut e = [Complex64::new(0.0, 0.0); 2];
        for (s, (w, r)) in z[a..b].iter().zip(tab) {
            let p = (s * c.conj()).arg() as f64;
            // Complex64::from_polar(p * w, θ), with θ's cosine and sine
            // from the table (the same values: no sine or cosine per
            // sample)
            for (k, r) in r.iter().enumerate() {
                e[k] += Complex64::new(p * w * r.re, p * w * r.im);
            }
        }
        let (p0, p1) = (e[0].norm_sqr(), e[1].norm_sqr());
        let q = ((p1 - p0).abs() / (p0 + p1).max(1e-30)) as f32;
        (u8::from(p1 > p0), q)
    }

    fn read(&mut self) {
        let end = self.base + self.z.len() as u64;
        if self.slot_phase.is_none() || end >= self.phase_at + 5 * FS2 as u64 {
            self.phase_at = end;
            if let Some(p) = self.find_slot_phase() {
                let moved = self.slot_phase.is_some_and(|q| {
                    let d = (p as i64 - q as i64).rem_euclid(SLOT as i64);
                    d.min(SLOT as i64 - d) > 4
                });
                if self.slot_phase.is_none() || moved {
                    // a new phase: read from here on
                    self.next_slot = None;
                    self.slots.clear();
                    self.marker = None;
                    self.secs.clear();
                    self.minute_at = None;
                }
                self.slot_phase = Some(p);
            }
        }
        let Some(p) = self.slot_phase else { return };
        let next = *self.next_slot.get_or_insert_with(|| {
            let s = self.base + (self.z.len() as u64).saturating_sub(10 * FS2 as u64);
            s + ((p as u64 + SLOT as u64 - s % SLOT as u64) % SLOT as u64)
        });
        let mut at = next.max(self.base);
        // follow a drifting phase by a sample or two
        let off = (at % SLOT as u64) as i64 - p as i64;
        if off != 0 && off.abs() <= 4 {
            at = (at as i64 - off) as u64;
        }
        while at + SLOT as u64 <= end {
            let i = (at - self.base) as usize;
            let (bit, q) = Self::slot_bit(&self.z[i..i + SLOT]);
            self.slots.push_back((at, bit, q));
            at += SLOT as u64;
            self.on_slot();
        }
        self.next_slot = Some(at);
        while self.slots.len() > 300 {
            self.slots.pop_front();
        }
    }

    /// The 900 ms slot: always 1, with the five always-0 slots 200-600 ms
    /// before it; from the last 20 s.
    fn find_marker(&self) -> Option<usize> {
        if self.slots.len() < 100 {
            return None;
        }
        let num = |at: u64| (at / SLOT as u64) as usize % 10;
        let mut ones = [0f32; 10];
        let mut cnt = [0f32; 10];
        for &(at, b, _) in self.slots.iter().rev().take(200) {
            ones[num(at)] += f32::from(b);
            cnt[num(at)] += 1.0;
        }
        let mean = |k: usize| ones[k % 10] / cnt[k % 10].max(1.0);
        let (best, score) = (0..10)
            .map(|r| {
                let zeros = (3..=7).map(|j| mean(r + 10 - j)).sum::<f32>() / 5.0;
                (r, mean(r) - zeros)
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))?;
        (score > 0.6).then_some(best)
    }

    fn on_slot(&mut self) {
        if self.slots.len().is_multiple_of(10) || self.marker.is_none() {
            let m = self.find_marker();
            if m != self.marker && m.is_some() {
                self.secs.clear();
                self.minute_at = None;
            }
            if m.is_some() {
                self.marker = m;
            }
        }
        let Some(mk) = self.marker else { return };
        let Some(&(at, _, _)) = self.slots.back() else {
            return;
        };
        // a second is complete at its 900 ms slot
        if (at / SLOT as u64) as usize % 10 != mk || self.slots.len() < 10 {
            return;
        }
        let s: Vec<(u64, u8, f32)> = self.slots.iter().rev().take(10).rev().copied().collect();
        let b: Vec<u8> = s.iter().map(|x| x.1).collect();
        // the fixed slots that read wrong lower the quality
        let wrong = (2..=6).filter(|&k| b[k] != 0).count() + usize::from(b[9] != 1);
        let q = s[0].2.min(s[1].2) * (1.0 - wrong as f32 / 6.0);
        let marker = b[7] == 1 && b[8] == 1;
        // as MSF's A and B: 0-3 = bit 1 + 2 * bit 2
        let sym = if marker {
            'M'
        } else {
            char::from(b'0' + b[0] + 2 * b[1])
        };
        let sec = Second {
            t: self.t_of(s[0].0),
            sym,
            quality: q,
            index: None,
            levels: s.iter().map(|x| f32::from(x.1)).collect(),
        };
        self.second(sec, b[0], b[1], marker);
    }

    fn second(&mut self, mut s: Second, d1: u8, d2: u8, marker: bool) {
        let t = s.t;
        self.secs.push((s.clone(), d1, d2));
        let n = self.secs.len();
        if marker {
            // second 59: the minute is the last 60 seconds
            if n >= 60 {
                let frame: Vec<(Second, u8, u8)> = self.secs[n - 60..].to_vec();
                if let Some(m) = self.minute(&frame, t + 1.0) {
                    self.out.push(Event::Minute(m));
                }
            }
            self.secs.clear();
            self.minute_at = Some(0);
            s.index = Some(59);
            self.out.push(Event::Second(s));
            return;
        }
        if self.secs.len() > 61 {
            self.secs.remove(0);
        }
        s.index = self
            .minute_at
            .map(|_| self.secs.len() - 1)
            .and_then(|i| u8::try_from(i).ok())
            .filter(|&i| i < 59);
        self.out.push(Event::Second(s));
    }

    fn minute(&mut self, frame: &[(Second, u8, u8)], next_t: f64) -> Option<Minute> {
        let d1: Vec<u8> = frame.iter().map(|x| x.1).collect();
        let d2: Vec<u8> = frame.iter().map(|x| x.2).collect();
        let symbols: String = frame.iter().map(|x| x.0.sym).collect();
        let mut m = decode(&d1, &d2, next_t)?;
        m.symbols = symbols;
        if m.ok {
            m.confirmed = self.passed.iter().any(|&(t, u)| {
                let k = ((m.t - t) / 60.0).round();
                k >= 1.0 && ((m.t - t) - 60.0 * k).abs() < 0.5 && m.utc - u == 60 * k as i64
            });
            self.passed.push((m.t, m.utc));
            self.passed.retain(|p| m.t - p.0 < 1200.0);
        }
        Some(m)
    }
}

fn num(b: &[u8], idx: &[(usize, i64)]) -> i64 {
    idx.iter().map(|&(i, w)| i64::from(b[i]) * w).sum()
}

fn xor(b: &[u8], r: std::ops::RangeInclusive<usize>) -> u8 {
    b[r].iter().fold(0, |a, &v| a ^ v)
}

/// The time-code fields (Wikipedia's table): data bit 1 and 2 of seconds
/// 0-59, second 59's 700/800 ms marker not included.
struct Fields {
    dut_sign_neg: bool,
    dut: i64,
    year: i64,
    month: i64,
    dow: i64,
    day: i64,
    hour: i64,
    minute: i64,
    tjd: i64,
    dut1: i64,
}

fn fields(d1: &[u8], d2: &[u8]) -> Fields {
    let dut1_pos = (1..=8).filter(|&i| d2[i] == 1).count() as i64;
    let dut1_neg = (9..=16).filter(|&i| d2[i] == 1).count() as i64;
    Fields {
        dut_sign_neg: d1[18] == 1,
        dut: num(d1, &[(19, 10), (20, 8), (21, 4), (22, 2), (23, 1)]),
        year: num(
            d1,
            &[
                (25, 80),
                (26, 40),
                (27, 20),
                (28, 10),
                (29, 8),
                (30, 4),
                (31, 2),
                (32, 1),
            ],
        ),
        month: num(d1, &[(33, 10), (34, 8), (35, 4), (36, 2), (37, 1)]),
        dow: num(d1, &[(38, 4), (39, 2), (40, 1)]),
        day: num(
            d1,
            &[(41, 20), (42, 10), (43, 8), (44, 4), (45, 2), (46, 1)],
        ),
        hour: num(
            d1,
            &[(47, 20), (48, 10), (49, 8), (50, 4), (51, 2), (52, 1)],
        ),
        minute: num(
            d1,
            &[
                (53, 40),
                (54, 20),
                (55, 10),
                (56, 8),
                (57, 4),
                (58, 2),
                (59, 1),
            ],
        ),
        tjd: num(
            d2,
            &[
                (18, 8000),
                (19, 4000),
                (20, 2000),
                (21, 1000),
                (22, 800),
                (23, 400),
                (24, 200),
                (25, 100),
                (26, 80),
                (27, 40),
                (28, 20),
                (29, 10),
                (30, 8),
                (31, 4),
                (32, 2),
                (33, 1),
            ],
        ),
        dut1: dut1_pos - dut1_neg,
    }
}

/// Even parity: each check bit makes its group's ones even.
const PARITY: [(usize, bool, std::ops::RangeInclusive<usize>); 8] = [
    // (second of the check bit in data bit 2, covers data bit 2?, seconds)
    (49, true, 18..=25),
    (50, true, 26..=33),
    (53, false, 18..=24),
    (54, false, 25..=32),
    (55, false, 33..=40),
    (56, false, 41..=46),
    (57, false, 47..=52),
    (58, false, 53..=59),
];

fn decode(d1: &[u8], d2: &[u8], next_t: f64) -> Option<Minute> {
    if d1.len() < 60 || d2.len() < 60 {
        return None;
    }
    let f = fields(d1, d2);
    let parity_ok = PARITY.iter().all(|(at, on2, r)| {
        let g = if *on2 {
            xor(d2, r.clone())
        } else {
            xor(d1, r.clone())
        };
        g == d2[*at]
    });
    let fixed_ok = d1[0] == 1
        && d2[0] == 1
        && [1, 2, 8, 9, 10, 16, 17, 24].iter().all(|&i| d1[i] == 0)
        && std::iter::once(17)
            .chain(34..=48)
            .chain([51, 52, 59])
            .all(|i| d2[i] == 0);
    let bcd = |v: i64, max: i64| v % 10 <= 9 && v <= max;
    let ranges_ok = bcd(f.year, 99)
        && (1..=12).contains(&f.month)
        && (1..=7).contains(&f.dow)
        && (1..=31).contains(&f.day)
        && f.hour <= 23
        && f.minute <= 59
        && f.dut % 10 <= 9;
    let y = 2000 + f.year;
    let days = days_from_civil(y, f.month, f.day);
    // the date as said: its day of week and (truncated MJD, of the local
    // or the UTC date) agree
    let dow_ok = (days + 3).rem_euclid(7) + 1 == f.dow;
    let mjd = days + 40_587;
    let tjd_ok = (-1..=0).any(|k| (mjd + k).rem_euclid(10_000) == f.tjd);
    let off = if f.dut_sign_neg { -f.dut } else { f.dut };
    let local = days * 86_400 + f.hour * 3600 + f.minute * 60;
    let utc = local - off * 3600;
    let (yy, mo, dd) = civil_from_days(local.div_euclid(86_400));
    let mut notes = Vec::new();
    if f.dut1 != 0 {
        notes.push(format!("DUT1 {:+.1} s", f.dut1 as f64 / 10.0));
    }
    if !tjd_ok {
        notes.push(format!("truncated MJD {} disagrees", f.tjd));
    }
    Some(Minute {
        station: Station::Rbu,
        t: next_t,
        utc,
        local: format!(
            "{yy:04}-{mo:02}-{dd:02} {:02}:{:02} UTC{off:+}",
            f.hour, f.minute
        ),
        ok: parity_ok && fixed_ok && ranges_ok && dow_ok,
        confirmed: false,
        notes,
        symbols: String::new(),
        named: true,
    })
}

/// The data bits of the minute starting at `utc` (UTC Unix seconds, a
/// whole minute): the Moscow time (UTC+3) of the next minute. For tests.
pub fn encode(utc: i64) -> ([u8; 60], [u8; 60]) {
    let mut d1 = [0u8; 60];
    let mut d2 = [0u8; 60];
    let local = utc + 60 + 3 * 3600;
    let days = local.div_euclid(86_400);
    let (y, mo, d) = civil_from_days(days);
    let secs = local.rem_euclid(86_400);
    let (h, mi) = (secs / 3600, secs / 60 % 60);
    let dow = (days + 3).rem_euclid(7) + 1;
    let tjd = (days + 40_587).rem_euclid(10_000);
    let put = |b: &mut [u8; 60], v: i64, idx: &[(usize, i64)]| {
        // BCD from the weights: the biggest first
        let mut tens = v / 10 * 10;
        let mut ones = v % 10;
        for &(i, w) in idx {
            if w >= 10 && tens >= w {
                b[i] = 1;
                tens -= w;
            } else if w < 10 && ones >= w {
                b[i] = 1;
                ones -= w;
            }
        }
    };
    d1[0] = 1;
    d2[0] = 1;
    put(&mut d1, 3, &[(19, 10), (20, 8), (21, 4), (22, 2), (23, 1)]);
    put(
        &mut d1,
        y % 100,
        &[
            (25, 80),
            (26, 40),
            (27, 20),
            (28, 10),
            (29, 8),
            (30, 4),
            (31, 2),
            (32, 1),
        ],
    );
    put(&mut d1, mo, &[(33, 10), (34, 8), (35, 4), (36, 2), (37, 1)]);
    put(&mut d1, dow, &[(38, 4), (39, 2), (40, 1)]);
    put(
        &mut d1,
        d,
        &[(41, 20), (42, 10), (43, 8), (44, 4), (45, 2), (46, 1)],
    );
    put(
        &mut d1,
        h,
        &[(47, 20), (48, 10), (49, 8), (50, 4), (51, 2), (52, 1)],
    );
    put(
        &mut d1,
        mi,
        &[
            (53, 40),
            (54, 20),
            (55, 10),
            (56, 8),
            (57, 4),
            (58, 2),
            (59, 1),
        ],
    );
    // the truncated MJD: four BCD digits
    for (k, digit) in [tjd / 1000, tjd / 100 % 10, tjd / 10 % 10, tjd % 10]
        .iter()
        .enumerate()
    {
        for (j, w) in [8, 4, 2, 1].iter().enumerate() {
            d2[18 + 4 * k + j] = u8::from(digit & w != 0);
        }
    }
    for (at, on2, r) in PARITY.iter() {
        let g = if *on2 {
            xor(&d2, r.clone())
        } else {
            xor(&d1, r.clone())
        };
        d2[*at] = g;
    }
    (d1, d2)
}

/// RBU as audio: its carrier at `tone` Hz in `fs` audio, from UTC
/// `start` (a whole second) for `secs`, phase-modulated as sent, with
/// white noise of rms `noise` (the carrier's amplitude is 1). For tests.
pub fn audio(start: i64, secs: usize, fs: f64, tone: f64, noise: f32) -> Vec<f32> {
    let n = (secs as f64 * fs) as usize;
    let mut out = Vec::with_capacity(n);
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut minute: Option<(i64, [u8; 60], [u8; 60])> = None;
    for k in 0..n {
        let t = k as f64 / fs;
        let utc = start as f64 + t;
        let sec = utc.floor() as i64;
        let m0 = sec - sec.rem_euclid(60);
        if minute.is_none_or(|m| m.0 != m0) {
            let (a, b) = encode(m0);
            minute = Some((m0, a, b));
        }
        let (_, d1, d2) = minute.expect("set");
        let s = sec.rem_euclid(60) as usize;
        let frac = utc - sec as f64;
        let slot = ((frac * 10.0) as usize).min(9);
        let in_slot = frac - slot as f64 / 10.0;
        let bit = match slot {
            0 => d1[s],
            1 => d2[s],
            7 | 8 => u8::from(s == 59),
            9 => 1,
            _ => 0,
        };
        let f = if bit == 1 { 312.5 } else { 100.0 };
        let (amp, pm) = if in_slot >= 0.095 {
            (0.0, 0.0)
        } else if (0.01..0.09).contains(&in_slot) {
            (1.0, 0.698 * (2.0 * PI * f * (in_slot - 0.01)).sin())
        } else {
            (1.0, 0.0)
        };
        // Box-Muller
        let g = (-2.0 * rnd().max(1e-12).ln()).sqrt() * (2.0 * PI * rnd()).cos();
        out.push((amp * (2.0 * PI * tone * t + pm).cos()) as f32 + noise * g as f32);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_code_reads_back() {
        // 2026-11-19 10:23 UTC
        let utc = days_from_civil(2026, 11, 19) * 86_400 + 10 * 3600 + 23 * 60;
        let (d1, d2) = encode(utc);
        let m = decode(&d1, &d2, 0.0).unwrap();
        assert!(m.ok, "{m:?}");
        assert_eq!(m.utc, utc + 60);
        assert_eq!(m.local, "2026-11-19 13:24 UTC+3");
        assert!(m.notes.is_empty(), "{:?}", m.notes);
        // a bit wrong: a parity fails
        let mut e = d1;
        e[55] ^= 1;
        assert!(!decode(&e, &d2, 0.0).unwrap().ok);
    }

    #[test]
    fn minutes_decode_from_the_signal() {
        let start = days_from_civil(2026, 11, 19) * 86_400 + 22 * 3600 + 7 * 60 + 40;
        let fs = 12_000.0;
        let x = audio(start, 200, fs, 1000.0, 0.3);
        let mut rx = Rbu::new(fs, 1000.0, 0.0);
        let mut minutes = vec![];
        let mut seconds = 0;
        for c in x.chunks(1200) {
            rx.push(c);
            for e in rx.poll() {
                match e {
                    Event::Minute(m) => minutes.push(m),
                    Event::Second(_) => seconds += 1,
                }
            }
        }
        assert!(seconds > 150, "{seconds} seconds");
        assert!(minutes.len() >= 2, "{minutes:?}");
        for m in &minutes {
            assert!(m.ok, "{m:?}");
            // the minute's time at its start, by the stream's clock
            let at = start as f64 + m.t;
            assert!(
                (at - m.utc as f64).abs() < 0.01,
                "{} s off",
                at - m.utc as f64
            );
        }
        assert!(minutes[1].confirmed);
        assert!(rx.level > 10.0, "level {}", rx.level);
    }
}
