// SPDX-License-Identifier: GPL-3.0-or-later
//! Frequency markers (labels over the spectrum) and band plans (the band
//! ribbon under it). The data is compiled into the binary (`data/`, its
//! sources in `data/README.md`); only the station's own list is read from
//! a file.
//!
//! Marker sources:
//! * `eibi`: EiBi's shortwave schedule for the season, filtered to what is
//!   on the air now (UTC time, weekday, dates);
//! * `utility`: time signals, marine, aero, weather fax, beacons;
//! * `amateur`: digital-mode dials, calling and emergency frequencies,
//!   per IARU region;
//! * `own`: the station's list (`own.rs`, a CSV file on the configuration
//!   partition).

pub mod bandplan;
pub mod eibi;
pub mod own;

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// What the receiver tunes with today. The signal's own kind (RTTY, fax,
/// FT8...) is in `Marker::emission`, for modes the receiver gets later.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tune {
    Usb,
    Lsb,
    Cw,
    Am,
}

/// A label on the spectrum.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    /// The signal's nominal (carrier) frequency, Hz.
    pub hz: f64,
    /// Where to put the dial if not on `hz` (e.g. USB below a fax carrier).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dial: Option<f64>,
    pub name: String,
    pub mode: Tune,
    /// The kind of signal, lowercase ("am", "cw", "rtty", "fax", "ft8"...).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub emission: String,
    /// What it is ("broadcast", "time", "marine", "digital"...).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub category: String,
    /// Published signal parameters (RTTY baud and shift, fax LPM...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// When it is on the air (UTC), as published.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schedule: String,
    /// The source list it comes from (`eibi`, `utility`, `amateur`, `own`).
    #[serde(default)]
    pub src: String,
}

/// The marker lists the receiver has.
pub const SOURCES: [&str; 4] = ["eibi", "utility", "amateur", "own"];

/// A marker as the bundled lists give it: the marker, the IARU regions it
/// applies to (empty: all) and the id of the document it comes from.
#[derive(Deserialize)]
struct ListMarker {
    #[serde(flatten)]
    marker: Marker,
    #[serde(default)]
    regions: Vec<u8>,
}

#[derive(Deserialize)]
struct List {
    markers: Vec<ListMarker>,
}

struct Listed {
    marker: Marker,
    regions: Vec<u8>,
}

fn load_list(src: &str, json: &str) -> Vec<Listed> {
    let list: List = serde_json::from_str(json).unwrap_or_else(|e| panic!("data/{src}.json: {e}"));
    let mut out: Vec<Listed> = list
        .markers
        .into_iter()
        .map(|l| Listed {
            marker: Marker {
                src: src.into(),
                ..l.marker
            },
            regions: l.regions,
        })
        .collect();
    out.sort_by(|a, b| a.marker.hz.total_cmp(&b.marker.hz));
    out
}

fn utility() -> &'static [Listed] {
    static L: OnceLock<Vec<Listed>> = OnceLock::new();
    L.get_or_init(|| load_list("utility", include_str!("../data/utility.json")))
}

fn amateur() -> &'static [Listed] {
    static L: OnceLock<Vec<Listed>> = OnceLock::new();
    L.get_or_init(|| load_list("amateur", include_str!("../data/amateur.json")))
}

/// UTC calendar time, enough for schedules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Utc {
    /// Minutes since midnight.
    pub minute: u16,
    /// 0 = Monday ... 6 = Sunday.
    pub weekday: u8,
    pub day: u8,
    pub month: u8,
}

impl Utc {
    /// From Unix time (seconds).
    pub fn from_unix(secs: i64) -> Utc {
        let days = secs.div_euclid(86_400);
        let minute = (secs.rem_euclid(86_400) / 60) as u16;
        // 1970-01-01 was a Thursday
        let weekday = (days + 3).rem_euclid(7) as u8;
        // days to civil date (H. Hinnant's algorithm)
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
        let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
        Utc {
            minute,
            weekday,
            day,
            month,
        }
    }

    pub fn now() -> Utc {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Utc::from_unix(secs)
    }
}

/// Which markers to give.
#[derive(Clone, Debug)]
pub struct Query<'a> {
    pub lo: f64,
    pub hi: f64,
    /// Source ids (`SOURCES`); unknown ones are ignored.
    pub sources: &'a [&'a str],
    /// IARU region (1-3) for the amateur list.
    pub region: u8,
    pub now: Utc,
    /// At most this many (the lowest frequencies first are kept).
    pub limit: usize,
}

/// The markers in `lo..=hi` from the chosen sources, sorted by frequency.
/// `own` is the station's list (already loaded).
pub fn query(q: &Query, own: &[Marker]) -> Vec<Marker> {
    let want = |s: &str| q.sources.contains(&s);
    let inside = |hz: f64| hz >= q.lo && hz <= q.hi;
    let mut out = Vec::new();
    if want("eibi") {
        out.extend(eibi::schedule().on_air(q.lo, q.hi, q.now));
    }
    let listed = |l: &&Listed| {
        inside(l.marker.hz) && (l.regions.is_empty() || l.regions.contains(&q.region))
    };
    if want("utility") {
        out.extend(utility().iter().filter(listed).map(|l| l.marker.clone()));
    }
    if want("amateur") {
        out.extend(amateur().iter().filter(listed).map(|l| l.marker.clone()));
    }
    if want("own") {
        out.extend(own.iter().filter(|m| inside(m.hz)).cloned());
    }
    out.sort_by(|a, b| a.hz.total_cmp(&b.hz));
    out.truncate(q.limit);
    out
}

/// How many markers each bundled list has (`eibi`: entries in the season).
pub fn counts() -> Vec<(&'static str, usize)> {
    vec![
        ("eibi", eibi::schedule().len()),
        ("utility", utility().len()),
        ("amateur", amateur().len()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_from_unix() {
        // 2026-11-04 15:30 UTC, a Wednesday
        let t = Utc::from_unix(1_793_806_200);
        assert_eq!(
            t,
            Utc {
                minute: 15 * 60 + 30,
                weekday: 2,
                day: 4,
                month: 11
            }
        );
        // 1970-01-01 00:00, a Thursday; 2000-02-29
        assert_eq!(
            Utc::from_unix(0),
            Utc {
                minute: 0,
                weekday: 3,
                day: 1,
                month: 1
            }
        );
        let leap = Utc::from_unix(951_782_400);
        assert_eq!((leap.day, leap.month, leap.weekday), (29, 2, 1));
    }

    #[test]
    fn bundled_lists_load() {
        for (src, n) in counts() {
            assert!(src != "eibi" || n > 1000, "{src}: {n}");
        }
    }
}
