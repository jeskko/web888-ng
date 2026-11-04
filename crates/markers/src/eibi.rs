// SPDX-License-Identifier: GPL-3.0-or-later
//! EiBi's shortwave schedule (eibispace.de, Eike Bierwirth: free to use,
//! copy and distribute, also within software; `data/README.md`). The
//! season's CSV database (`sked-<season>.csv`) is compiled in; the field
//! meanings are those of EiBi's README.TXT, section C.
//!
//! Each line: kHz; time (UTC, "hhmm-hhmm"); days; ITU country; station;
//! language (or a special code: -CW Morse, -TS time signal, -TY teletype or
//! other digital, -HF HFDL, -MX music, -EC empty carrier); target area;
//! remarks (transmitter site); persistence; start; stop.

use crate::{Marker, Tune, Utc};
use std::sync::OnceLock;

/// The bundled season: its CSV, and whether it is a winter (B) season.
const CSV: &[u8] = include_bytes!("../data/eibi-b26.csv");
pub const SEASON: &str = "B26";
const WINTER: bool = true;

/// Days of operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Days {
    /// Every day (also "irr", "test", "tent": may be on, shown).
    Daily,
    /// These weekdays (bit 0 = Monday).
    Week(u8),
    /// Not normally on the air ("alt", "harm", "imod").
    Off,
}

impl Days {
    pub fn parse(s: &str) -> Days {
        let s = s.trim();
        if s.is_empty() {
            return Days::Daily;
        }
        let l = s.to_ascii_lowercase();
        if l.starts_with("alt") || l == "harm" || l == "imod" {
            return Days::Off;
        }
        // 1234567: weekday numbers, 1 = Monday
        if s.bytes().all(|b| (b'1'..=b'7').contains(&b)) {
            return Days::Week(s.bytes().fold(0, |m, b| m | 1 << (b - b'1')));
        }
        // two-letter days, joined ("SaSu"), listed ("Tu,Fr") or ranges
        // ("Mo-Fr", "We-Mo" wrapping); "1.Sa" (first Saturday) counts as
        // the weekday
        const NAMES: [&str; 7] = ["mo", "tu", "we", "th", "fr", "sa", "su"];
        let day = |t: &str| NAMES.iter().position(|n| *n == t);
        let mut mask = 0u8;
        let core = l.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.');
        for part in core.split(',') {
            let part = part.trim();
            if let Some((a, b)) = part.split_once('-') {
                let (Some(a), Some(b)) = (day(a), day(b)) else {
                    return Days::Daily;
                };
                let mut d = a;
                loop {
                    mask |= 1 << d;
                    if d == b {
                        break;
                    }
                    d = (d + 1) % 7;
                }
            } else if part.len() % 2 == 0 && !part.is_empty() {
                for i in (0..part.len()).step_by(2) {
                    match part.get(i..i + 2).and_then(day) {
                        Some(d) => mask |= 1 << d,
                        None => return Days::Daily,
                    }
                }
            } else {
                return Days::Daily;
            }
        }
        if mask == 0 {
            Days::Daily
        } else {
            Days::Week(mask)
        }
    }

    fn on(self, weekday: u8) -> bool {
        match self {
            Days::Daily => true,
            Days::Week(m) => m & (1 << weekday) != 0,
            Days::Off => false,
        }
    }
}

/// One schedule line.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub khz: f64,
    /// Minutes since midnight UTC; `to` may be below `from` (over midnight).
    pub from: u16,
    pub to: u16,
    pub days: Days,
    pub days_text: String,
    pub country: String,
    pub station: String,
    pub language: String,
    pub target: String,
    pub site: String,
    pub persistence: u8,
    /// Dates (day, month) for persistence 6.
    pub start: Option<(u8, u8)>,
    pub stop: Option<(u8, u8)>,
}

fn hhmm(s: &str) -> Option<u16> {
    let v: u16 = s.parse().ok()?;
    let (h, m) = (v / 100, v % 100);
    (h <= 24 && m < 60 && v <= 2400).then_some(h * 60 + m)
}

/// "2810": 28 October; "[0923]" (last heard) is not a date.
fn ddmm(s: &str) -> Option<(u8, u8)> {
    let s = s.trim();
    if s.len() != 4 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (d, m) = (s[..2].parse().ok()?, s[2..].parse().ok()?);
    ((1..=31).contains(&d) && (1..=12).contains(&m)).then_some((d, m))
}

impl Entry {
    pub fn parse(line: &str) -> Option<Entry> {
        let f: Vec<&str> = line.trim_end_matches(['\r', '\n']).split(';').collect();
        if f.len() < 9 {
            return None;
        }
        let khz: f64 = f[0].trim().parse().ok()?;
        let (a, b) = f[1].trim().split_once('-')?;
        let persistence: u8 = f[8].trim().parse().ok()?;
        Some(Entry {
            khz,
            from: hhmm(a)?,
            to: hhmm(b)?,
            days: Days::parse(f[2]),
            days_text: f[2].trim().into(),
            country: f[3].trim().into(),
            station: f[4].trim().into(),
            language: f[5].trim().into(),
            target: f[6].trim().into(),
            site: f[7].trim().into(),
            persistence,
            start: f.get(9).and_then(|s| ddmm(s)),
            stop: f.get(10).and_then(|s| ddmm(s)),
        })
    }

    /// Within its time window (the end minute excluded; "0000-2400" is
    /// all day, "2300-0100" crosses midnight).
    fn in_window(&self, minute: u16) -> bool {
        if self.from == self.to || (self.from == 0 && self.to == 1440) {
            return true;
        }
        if self.from < self.to {
            minute >= self.from && minute < self.to
        } else {
            minute >= self.from || minute < self.to
        }
    }

    /// On the air at `t`, as far as the schedule says.
    pub fn on_air(&self, t: Utc) -> bool {
        let seasonal = match self.persistence % 90 {
            8 => false,
            4 => WINTER,
            5 => !WINTER,
            6 => match (self.start, self.stop) {
                (Some(a), Some(b)) => between((t.day, t.month), a, b),
                _ => true,
            },
            _ => true,
        };
        if !seasonal || !self.in_window(t.minute) {
            return false;
        }
        // over midnight: the days are those it starts on
        let day = if self.from > self.to && t.minute < self.to {
            (t.weekday + 6) % 7
        } else {
            t.weekday
        };
        self.days.on(day)
    }

    pub fn marker(&self) -> Marker {
        let (mode, emission, category) = match self.language.as_str() {
            "-CW" => (Tune::Cw, "cw", "utility"),
            "-TS" => (Tune::Am, "time", "time"),
            "-TY" => (Tune::Usb, "data", "utility"),
            "-HF" => (Tune::Usb, "hfdl", "aero"),
            _ => (Tune::Am, "am", "broadcast"),
        };
        let lang = if self.language.starts_with('-') {
            ""
        } else {
            &self.language
        };
        let mut note = String::new();
        for (k, v) in [("", lang), ("to ", &self.target), ("", &self.country)] {
            if !v.is_empty() {
                if !note.is_empty() {
                    note.push(' ');
                }
                note.push_str(k);
                note.push_str(v);
            }
        }
        let mut schedule = format!(
            "{:02}{:02}-{:02}{:02}",
            self.from / 60,
            self.from % 60,
            self.to / 60,
            self.to % 60
        );
        if !self.days_text.is_empty() {
            schedule.push(' ');
            schedule.push_str(&self.days_text);
        }
        Marker {
            hz: self.khz * 1000.0,
            dial: None,
            name: self.station.clone(),
            mode,
            emission: emission.into(),
            category: category.into(),
            params: None,
            note,
            schedule,
            src: "eibi".into(),
        }
    }
}

/// `t` within `a..=b` (day, month), the range wrapping over New Year.
fn between(t: (u8, u8), a: (u8, u8), b: (u8, u8)) -> bool {
    let k = |(d, m): (u8, u8)| m as u16 * 32 + d as u16;
    let (t, a, b) = (k(t), k(a), k(b));
    if a <= b {
        t >= a && t <= b
    } else {
        t >= a || t <= b
    }
}

/// The season's schedule, sorted by frequency.
pub struct Schedule {
    entries: Vec<Entry>,
}

impl Schedule {
    /// From EiBi's CSV (ISO 8859-1, a header line).
    pub fn parse(csv: &[u8]) -> Schedule {
        // ISO 8859-1: each byte is its code point
        let text: String = csv.iter().map(|&b| b as char).collect();
        let mut entries: Vec<Entry> = text.lines().skip(1).filter_map(Entry::parse).collect();
        entries.sort_by(|a, b| a.khz.total_cmp(&b.khz));
        Schedule { entries }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// What is on the air in `lo..=hi` (Hz) at `t`; one marker per
    /// frequency and station.
    pub fn on_air(&self, lo: f64, hi: f64, t: Utc) -> Vec<Marker> {
        let first = self.entries.partition_point(|e| e.khz * 1000.0 < lo);
        let mut out: Vec<Marker> = Vec::new();
        for e in &self.entries[first..] {
            if e.khz * 1000.0 > hi {
                break;
            }
            if !e.on_air(t) {
                continue;
            }
            let m = e.marker();
            if out.last().is_some_and(|p| p.hz == m.hz && p.name == m.name) {
                continue;
            }
            out.push(m);
        }
        out
    }
}

pub fn schedule() -> &'static Schedule {
    static S: OnceLock<Schedule> = OnceLock::new();
    S.get_or_init(|| Schedule::parse(CSV))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(weekday: u8, hhmm: u16, day: u8, month: u8) -> Utc {
        Utc {
            minute: hhmm / 100 * 60 + hhmm % 100,
            weekday,
            day,
            month,
        }
    }

    #[test]
    fn days() {
        assert_eq!(Days::parse(""), Days::Daily);
        assert_eq!(Days::parse("Mo-Fr"), Days::Week(0b0011111));
        assert_eq!(Days::parse("We-Mo"), Days::Week(0b1111101)); // all but Tuesday
        assert_eq!(Days::parse("SaSu"), Days::Week(0b1100000));
        assert_eq!(Days::parse("Tu,Fr"), Days::Week(0b0010010));
        assert_eq!(Days::parse("1245"), Days::Week(0b0011011));
        assert_eq!(Days::parse("1.Sa"), Days::Week(0b0100000));
        assert_eq!(Days::parse("irr"), Days::Daily);
        assert_eq!(Days::parse("alt"), Days::Off);
        assert_eq!(Days::parse("harm"), Days::Off);
    }

    #[test]
    fn windows_and_dates() {
        let e = Entry::parse("6070;2300-0100;Sa;D;Channel 292;E;Eu;r;1;;").unwrap();
        assert!(e.on_air(at(5, 2330, 1, 11))); // Saturday 2330
        assert!(e.on_air(at(6, 30, 2, 11))); // Sunday 0030: Saturday's broadcast
        assert!(!e.on_air(at(5, 30, 1, 11))); // Saturday 0030: Friday's, none
        assert!(!e.on_air(at(5, 100, 1, 11))); // the end minute is out
        let all = Entry::parse("9420;0000-2400;;GRC;ERT;Gr;Eu;a;1;;").unwrap();
        assert!(all.on_air(at(3, 1234, 5, 11)));
        // persistence 6: only between the dates (28 Oct - 23 Dec)
        let dated = Entry::parse("1386;1430-1500;23Dec;D;SM Radio Dessau;D;EEu;/LTU-v;6;2810;2312")
            .unwrap();
        assert_eq!((dated.start, dated.stop), (Some((28, 10)), Some((23, 12))));
        assert!(dated.on_air(at(2, 1445, 4, 11)));
        assert!(!dated.on_air(at(2, 1445, 24, 12)));
        // a date range over New Year
        assert!(between((5, 1), (1, 12), (31, 1)));
        assert!(!between((5, 2), (1, 12), (31, 1)));
        // inactive (8) and summer-only (5) in a winter file
        assert!(
            !Entry::parse("5000;0000-2400;;X;Y;E;Eu;a;8;;")
                .unwrap()
                .on_air(at(0, 1200, 1, 11))
        );
        assert!(
            !Entry::parse("5000;0000-2400;;X;Y;E;Eu;a;5;;")
                .unwrap()
                .on_air(at(0, 1200, 1, 11))
        );
        assert!(
            Entry::parse("5000;0000-2400;;X;Y;E;Eu;a;4;;")
                .unwrap()
                .on_air(at(0, 1200, 1, 11))
        );
        // "[0923]" is when it was last heard, not a stop date
        assert_eq!(
            Entry::parse("16.4;0000-2400;;NOR;JXN;;NAO;no;1;;[0923]")
                .unwrap()
                .stop,
            None
        );
    }

    #[test]
    fn markers_from_codes() {
        let cw = Entry::parse("18.1;0000-2400;;RUS;RDL Russian Navy;-CW;RUS;m;1;;")
            .unwrap()
            .marker();
        assert_eq!(
            (cw.hz, cw.mode, cw.emission.as_str()),
            (18_100.0, Tune::Cw, "cw")
        );
        let ts = Entry::parse("4996;0000-2400;;RUS;RWM;-TS;Eu;m;1;;")
            .unwrap()
            .marker();
        assert_eq!((ts.mode, ts.category.as_str()), (Tune::Am, "time"));
        let bc = Entry::parse("9420;1600-1700;Mo-Fr;GRC;ERT;Gr;Eu;a;1;;")
            .unwrap()
            .marker();
        assert_eq!(
            (bc.mode, bc.schedule.as_str(), bc.note.as_str()),
            (Tune::Am, "1600-1700 Mo-Fr", "Gr to Eu GRC")
        );
    }

    #[test]
    fn the_bundled_season() {
        let s = schedule();
        assert!(s.len() > 8000, "{}", s.len());
        // a weekday afternoon: plenty on 49 m, nothing outside the range
        let m = s.on_air(5_900_000.0, 6_200_000.0, at(2, 1500, 4, 11));
        assert!(m.len() > 20, "{}", m.len());
        assert!(
            m.iter()
                .all(|x| (5_900_000.0..=6_200_000.0).contains(&x.hz))
        );
        assert!(m.windows(2).all(|w| w[0].hz <= w[1].hz));
    }
}
