// SPDX-License-Identifier: GPL-3.0-or-later
//! EFR's LF teleswitch stations (DCF39 139.0 kHz, HGA22 135.6 kHz,
//! DCF49 129.1 kHz): FSK ±170 Hz at 200 Bd, idle on the low tone (a 1),
//! UART 8E1, IEC 60870-5 FT1.2 frames. Every 10 s a time telegram,
//! measured on the remote unit on 2026-11-19 (DCF39's start bit 1206.41 ms
//! after each 10 s mark, ±0.14 ms):
//!
//! `68 0A 0A 68 | C 00 00 00 | q min hour (dow<<5 | day) month year | CS 16`
//!
//! `C`: a counter in the high nibble, 7 in the low; `q`: quarter seconds
//! of the minute, of the mark + 2 s (the telegram's own time); the time
//! is the German legal time (CET; CEST in summer by the EU rule:
//! assumed, only winter heard yet); `CS`: the data bytes' sum.
//!
//! The receiver: the audio's tone mixed to 0 Hz, two 5 ms matched filters
//! (one bit) at -170 and +170 Hz, their difference over their sum (`m`,
//! positive for the low tone: a 1). A frame starts where `m` turns to a 0 (the
//! start bit) after idling at 1: the header read at the bits' centres by
//! the frame's own timing (byte q at 55 q ms: the station's clock, not
//! each start bit's), a parity error mended by flipping the least certain
//! bit (the checksum confirms), and the start edge from correlating the
//! whole frame's bits with `m`. Each time telegram: a [`Minute`] (as the
//! other stations' minutes: `t` the start bit's stream time, `utc` the
//! 10 s mark it belongs to), confirmed by an earlier one that agrees.

use crate::{Event, Minute, Station, civil_from_days, days_from_civil};
use num_complex::{Complex32, Complex64};
use std::f64::consts::PI;

/// The tones' offsets from the carrier (Hz).
const SHIFT: f64 = 170.0;
const BAUD: f64 = 200.0;
/// The time telegram's bytes.
const TIME_LEN: usize = 16;
/// Kept: a few seconds more than a telegram.
const KEEP_S: f64 = 4.0;

pub struct Efr {
    station: Station,
    fs: f64,
    t0: f64,
    /// Samples per bit, per byte (11 bits).
    spb: usize,
    /// The mixer to 0 Hz and the tones' to 0 Hz (rotators).
    lo: Complex64,
    lo_step: Complex64,
    up: Complex64,
    up_step: Complex64,
    /// The matched filters: the last `spb` samples at each tone and
    /// their sums.
    ring: Vec<(Complex32, Complex32)>,
    pos: usize,
    sum: (Complex64, Complex64),
    n: u64,
    /// The metric per sample: `m[j]` the filters over samples j - spb/2
    /// .. j + spb/2 (centred, as the frame's edges are found); index
    /// `base` is m[0]; and the powers (low, high) for the level.
    m: Vec<f32>,
    base: u64,
    /// The next index to look for a start bit at.
    next: u64,
    pw: (f64, f64),
    /// Telegrams that passed (stream time, UTC) for confirming.
    passed: Vec<(f64, i64)>,
    out: Vec<Event>,
    /// The tones' contrast over the last second (dB): how well it reads.
    pub level: f32,
}

impl Efr {
    pub fn new(station: Station, fs: f64, tone: f64, t0: f64) -> Self {
        let spb = (fs / BAUD).round() as usize;
        Efr {
            station,
            fs,
            t0,
            spb,
            lo: Complex64::new(1.0, 0.0),
            lo_step: Complex64::from_polar(1.0, -2.0 * PI * tone / fs),
            up: Complex64::new(1.0, 0.0),
            up_step: Complex64::from_polar(1.0, 2.0 * PI * SHIFT / fs),
            ring: vec![Default::default(); spb],
            pos: 0,
            sum: Default::default(),
            n: 0,
            // the first spb/2 indices have no full window
            m: vec![0.0; spb / 2],
            base: 0,
            next: 1,
            pw: (0.0, 0.0),
            passed: Vec::new(),
            out: Vec::new(),
            level: 0.0,
        }
    }

    pub fn set_tone(&mut self, tone: f64) {
        self.lo_step = Complex64::from_polar(1.0, -2.0 * PI * tone / self.fs);
    }

    pub fn poll(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.out)
    }

    pub fn push(&mut self, x: &[f32]) {
        let w = self.spb;
        let a_pw = 1.0 / self.fs;
        for &v in x {
            let z = self.lo * v as f64;
            self.lo *= self.lo_step;
            // the low tone (-170 Hz) up to 0, the high one down
            let zl = z * self.up;
            let zh = z * self.up.conj();
            self.up *= self.up_step;
            let (zl, zh) = (
                Complex32::new(zl.re as f32, zl.im as f32),
                Complex32::new(zh.re as f32, zh.im as f32),
            );
            let old = self.ring[self.pos];
            self.ring[self.pos] = (zl, zh);
            self.pos = (self.pos + 1) % w;
            self.sum.0 += Complex64::new((zl.re - old.0.re) as f64, (zl.im - old.0.im) as f64);
            self.sum.1 += Complex64::new((zh.re - old.1.re) as f64, (zh.im - old.1.im) as f64);
            self.n += 1;
            if self.n >= w as u64 {
                let (l, h) = (self.sum.0.norm_sqr(), self.sum.1.norm_sqr());
                self.m.push(((l - h) / (l + h).max(1e-30)) as f32);
                // the level: each tone's power, then max over min
                let (hi, lo) = if l > h { (l, h) } else { (h, l) };
                self.pw.0 += (hi - self.pw.0) * a_pw;
                self.pw.1 += (lo - self.pw.1) * a_pw;
            }
        }
        self.lo /= self.lo.norm();
        self.up /= self.up.norm();
        // re-sum now and then (rounding)
        if self.n % (self.fs as u64 * 10) < x.len() as u64 {
            let (a, b) = self.ring.iter().fold(
                (Complex64::default(), Complex64::default()),
                |(a, b), (l, h)| {
                    (
                        a + Complex64::new(l.re as f64, l.im as f64),
                        b + Complex64::new(h.re as f64, h.im as f64),
                    )
                },
            );
            self.sum = (a, b);
        }
        self.level = (10.0 * (self.pw.0 / self.pw.1.max(1e-30)).log10()) as f32;
        self.scan();
        let keep = (KEEP_S * self.fs) as usize;
        let end = self.base + self.m.len() as u64;
        let drop_to = self.next.min(end).saturating_sub(keep as u64);
        if drop_to > self.base + keep as u64 {
            let d = (drop_to - self.base) as usize;
            self.m.drain(..d);
            self.base += d as u64;
        }
    }

    fn at(&self, j: u64) -> f32 {
        self.m[(j - self.base) as usize]
    }

    /// Byte `q` of a frame starting at `i0` (+ fractional samples): data,
    /// whether its parity needed mending, and its start and stop bits
    /// right; none past the samples there are.
    fn byte(&self, i0: u64, q: usize) -> Option<(u8, bool, bool)> {
        let s = self.spb as f64;
        let c = |k: usize| i0 + ((q * 11 + k) as f64 * s + s / 2.0) as u64;
        if c(10) >= self.base + self.m.len() as u64 {
            return None;
        }
        let bits: Vec<f32> = (1..=9).map(|k| self.at(c(k))).collect();
        let mut b: Vec<bool> = bits.iter().map(|&v| v > 0.0).collect();
        let mut fixed = false;
        // even parity (8 data + parity); broken: flip the least certain
        if b.iter().filter(|&&x| x).count() % 2 == 1 {
            let j = (0..9)
                .min_by(|&a, &b| bits[a].abs().total_cmp(&bits[b].abs()))
                .unwrap_or(0);
            b[j] = !b[j];
            fixed = true;
        }
        let v = (0..8).fold(0u8, |v, k| v | (u8::from(b[k]) << k));
        let framing = self.at(c(0)) <= 0.0 && self.at(c(10)) >= 0.0;
        Some((v, fixed, framing))
    }

    /// The bytes of `n` from `i0`, and whether every start and stop bit
    /// was right.
    fn bytes(&self, i0: u64, n: usize) -> Option<(Vec<u8>, bool)> {
        let mut v = Vec::with_capacity(n);
        let mut ok = true;
        for q in 0..n {
            let (b, _, f) = self.byte(i0, q)?;
            v.push(b);
            ok &= f;
        }
        Some((v, ok))
    }

    fn scan(&mut self) {
        let spb = self.spb as u64;
        let byte = 11 * spb;
        let end = self.base + self.m.len() as u64;
        while self.next + (TIME_LEN as u64 + 1) * byte < end {
            let i = self.next.max(self.base + 1);
            self.next = i + 1;
            // a start bit after a 1
            if !(self.at(i) < 0.0 && self.at(i - 1) >= 0.0) {
                continue;
            }
            let half = spb / 2;
            let zeros = (i..i + half).filter(|&j| self.at(j) < 0.0).count();
            if zeros * 5 < half as usize * 4 {
                continue;
            }
            let Some((h, _)) = self.bytes(i, 4) else {
                return;
            };
            if !(h[0] == 0x68 && h[3] == 0x68 && h[1] == h[2]) {
                continue;
            }
            let n = h[1] as usize + 6;
            if n != TIME_LEN {
                // another telegram: past it
                self.next = i + n as u64 * byte;
                continue;
            }
            // the frame's timing: where its bits' centres read clearest
            let score = |i0: u64| -> f32 {
                (0..n * 11)
                    .map(|k| {
                        self.at(i0 + (k as f64 * spb as f64 + spb as f64 / 2.0) as u64)
                            .abs()
                    })
                    .sum()
            };
            let i0 = (i.saturating_sub(spb / 4)..=i + spb / 4)
                .filter(|&j| j > self.base)
                .max_by(|&a, &b| score(a).total_cmp(&score(b)))
                .unwrap_or(i);
            let Some((v, framing)) = self.bytes(i0, n) else {
                return;
            };
            self.next = i0 + n as u64 * byte - spb;
            let sum = v[4..14].iter().fold(0u8, |a, &b| a.wrapping_add(b));
            if !(framing && v[15] == 0x16 && v[14] == sum) {
                continue;
            }
            let Some((utc, local)) = telegram_time(&v) else {
                continue;
            };
            let edge = self.edge(i0, &v);
            let t = self.t0 + edge / self.fs;
            let confirmed = self.passed.iter().any(|&(t1, u1)| {
                let k = ((t - t1) / 10.0).round();
                k >= 1.0 && ((t - t1) - 10.0 * k).abs() < 0.5 && utc - u1 == 10 * k as i64
            });
            self.passed.push((t, utc));
            self.passed.retain(|p| t - p.0 < 600.0);
            self.out.push(Event::Minute(Minute {
                station: self.station,
                t,
                utc,
                local,
                ok: true,
                confirmed,
                notes: vec![format!("C {:02X}", v[4])],
                symbols: String::new(),
                named: true,
            }));
        }
    }

    /// The frame's start edge (samples, fractional): the bits as sent
    /// (+1 for a 1) correlated with `m` around `i0`, the peak's parabola.
    fn edge(&self, i0: u64, v: &[u8]) -> f64 {
        let spb = self.spb;
        let mut tpl: Vec<f32> = Vec::with_capacity(v.len() * 11 * spb);
        for &b in v {
            let par = b.count_ones() % 2 == 1;
            let bits = std::iter::once(false)
                .chain((0..8).map(|k| (b >> k) & 1 == 1))
                .chain([par, true]);
            for bit in bits {
                tpl.extend(std::iter::repeat_n(if bit { 1.0 } else { -1.0 }, spb));
            }
        }
        let r = (spb / 2) as i64;
        let end = self.base + self.m.len() as u64;
        let c: Vec<(i64, f64)> = (-r..=r)
            .filter_map(|d| {
                let s = i0 as i64 + d;
                (s > self.base as i64 && (s as u64 + tpl.len() as u64) < end).then(|| {
                    let s = s as u64;
                    let v: f64 = tpl
                        .iter()
                        .enumerate()
                        .map(|(k, &t)| (t * self.at(s + k as u64)) as f64)
                        .sum();
                    (d, v)
                })
            })
            .collect();
        let Some(k) = (0..c.len()).max_by(|&a, &b| c[a].1.total_cmp(&c[b].1)) else {
            return i0 as f64;
        };
        let frac = if k > 0 && k + 1 < c.len() {
            let (y0, y1, y2) = (c[k - 1].1, c[k].1, c[k + 1].1);
            let den = y0 - 2.0 * y1 + y2;
            if den.abs() > 1e-12 {
                0.5 * (y0 - y2) / den
            } else {
                0.0
            }
        } else {
            0.0
        };
        i0 as f64 + c[k].0 as f64 + frac
    }
}

/// The time telegram's 10 s mark (UTC) and its own words for it; none
/// when a field is out of range or the bytes aren't a time telegram.
pub fn telegram_time(v: &[u8]) -> Option<(i64, String)> {
    if v.len() != TIME_LEN || v[4] & 0x0F != 7 || v[5..8] != [0, 0, 0] {
        return None;
    }
    let (q, min, hour, dd, mon, yy) = (v[8], v[9], v[10], v[11], v[12], v[13]);
    let day = (dd & 0x1F) as i64;
    if q % 4 != 0 || q / 4 >= 60 || min >= 60 || hour >= 24 || !(1..=31).contains(&day) {
        return None;
    }
    if !(1..=12).contains(&mon) {
        return None;
    }
    let sec = (q / 4) as i64;
    let local = days_from_civil(2000 + yy as i64, mon as i64, day) * 86_400
        + hour as i64 * 3600
        + min as i64 * 60
        + sec;
    // German legal time: CEST when that falls in the EU's summer time
    let (utc, zone) = if summer(local - 7200) {
        (local - 7200, "CEST")
    } else {
        (local - 3600, "CET")
    };
    // the telegram's time is the mark + 2 s
    let mark = utc - 2;
    if mark.rem_euclid(10) != 0 {
        return None;
    }
    Some((
        mark,
        format!("20{yy:02}-{mon:02}-{day:02} {hour:02}:{min:02}:{sec:02} {zone}"),
    ))
}

/// EU summer time at `utc` (s): from the last Sunday of March 01:00 UTC
/// to the last Sunday of October 01:00 UTC.
fn summer(utc: i64) -> bool {
    let (y, _, _) = civil_from_days(utc.div_euclid(86_400));
    let last_sunday = |m: i64| {
        let d = days_from_civil(y, m, 31);
        // 1970-01-01 was a Thursday (4)
        let dow = (d + 4).rem_euclid(7);
        (d - dow) * 86_400 + 3600
    };
    utc >= last_sunday(3) && utc < last_sunday(10)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 10:55:00 UTC on 2026-11-19, as DCF39 sent it.
    const T: [u8; 16] = [
        0x68, 0x0a, 0x0a, 0x68, 0x77, 0, 0, 0, 0x08, 0x37, 0x0b, 0x93, 0x0b, 0x1a, 0x79, 0x16,
    ];

    #[test]
    fn the_telegram_reads() {
        let (u, local) = telegram_time(&T).unwrap();
        assert_eq!(u, 1_795_085_700);
        assert_eq!(local, "2026-11-19 11:55:02 CET");
        // summer: CEST
        let jul = days_from_civil(2026, 7, 1) * 86_400 + 12 * 3600;
        assert!(summer(jul));
        assert!(!summer(1_795_085_700));
    }

    /// FSK as sent, in noise: the telegrams decode, confirmed from the
    /// second on, their start edges within a fraction of a sample.
    #[test]
    fn telegrams_decode_and_time() {
        let fs = 4000.0;
        let tone = 1000.0;
        let mark0: i64 = 1_795_085_700;
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64 - 0.5
        };
        // telegrams at mark + 1.2064 s, for 5 marks; stream time 0 = mark0 - 3.3
        let t0 = mark0 as f64 - 3.3;
        // on a sample (the bits are drawn on whole samples)
        let start = 1.2065;
        let n = (60.0 * fs) as usize;
        let mut bits_at = vec![1i8; n];
        for k in 0..5i64 {
            let mut v = T;
            // q = 4 (s + 2) for the mark's second, and the checksum
            let sec = (k * 10) % 60;
            v[8] = (4 * (sec + 2)) as u8;
            v[4] = 0x07 | ((k as u8 & 0xF) << 4);
            v[14] = v[4..14].iter().fold(0u8, |a, &b| a.wrapping_add(b));
            let s0 = ((mark0 + 10 * k) as f64 + start - t0) * fs;
            let mut bit = 0usize;
            for &b in &v {
                let par = b.count_ones() % 2 == 1;
                let bs: Vec<bool> = std::iter::once(false)
                    .chain((0..8).map(|j| (b >> j) & 1 == 1))
                    .chain([par, true])
                    .collect();
                for x in bs {
                    let a = (s0 + bit as f64 * fs / BAUD).round() as usize;
                    let e = (s0 + (bit + 1) as f64 * fs / BAUD).round() as usize;
                    for s in &mut bits_at[a..e] {
                        *s = if x { 1 } else { -1 };
                    }
                    bit += 1;
                }
            }
        }
        let mut ph = 0.0f64;
        let audio: Vec<f32> = bits_at
            .iter()
            .map(|&b| {
                // 1: the low tone
                let f = tone - SHIFT * b as f64;
                ph += 2.0 * PI * f / fs;
                (0.1 * ph.cos() + 0.05 * rnd()) as f32
            })
            .collect();
        let mut rx = Efr::new(Station::Dcf39, fs, tone, t0);
        let mut got = Vec::new();
        for c in audio.chunks(4000) {
            rx.push(c);
            got.extend(rx.poll());
        }
        let ms: Vec<Minute> = got
            .into_iter()
            .filter_map(|e| {
                if let Event::Minute(m) = e {
                    Some(m)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(ms.len(), 5, "{ms:#?}");
        for (k, m) in ms.iter().enumerate() {
            assert_eq!(m.utc, mark0 + 10 * k as i64);
            assert_eq!(m.confirmed, k > 0);
            let late = m.t - m.utc as f64;
            assert!((late - start).abs() < 0.05e-3, "{k}: {late}");
        }
        assert!(rx.level > 10.0, "{}", rx.level);
    }
}
