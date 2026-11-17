// SPDX-License-Identifier: GPL-3.0-or-later
//! HF propagation forecasts (`doc/m2-web-ui.md` §13): VOACAP, through
//! the hfcast port, from a few regions of the world to the receiver.
//!
//! A forecast is the month's median for each UTC hour: per path the MUF
//! and LUF, and per band the reliability (the share of days the signal
//! meets the required SNR) and the median SNR.
//!
//! The one input that changes is the sunspot number, the month's
//! smoothed value. It comes, in this order, from the configuration (a
//! fixed number), from NOAA's predicted solar cycle when the receiver
//! can fetch it (kept on disk between fetches, so a receiver that loses
//! its connection keeps the last table), or from the table built into
//! hfcast (to 2030; held at its last value after that). So a receiver
//! with no network at all still forecasts.
//!
//! The engine runs on the caller's thread: about 0.2 s per path on the
//! receiver. Any panic in it is caught and reported as an error for that
//! path.

use serde::Serialize;
use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};

/// The bands the engine takes at most (VOACAP's eleven frequency slots).
pub const MAX_BANDS: usize = 11;

/// Which model answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    /// VOACAP as it is: the month's median.
    Voacap,
    /// hfcast's Truecast: the same physics, at a daily sunspot index
    /// (the month's value with hfcast's fitted day-of-year correction).
    Truecast,
}

/// Where a forecast's sunspot number came from.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase", tag = "kind")]
pub enum SsnSource {
    /// `[propagation] ssn`.
    Config,
    /// NOAA SWPC's predicted solar cycle, fetched at `fetched` (UTC
    /// seconds).
    Noaa { fetched: u64 },
    /// The table built into hfcast.
    Builtin,
}

/// A region the forecast is for (signals from there to the receiver).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Target {
    pub name: String,
    pub lat: f64,
    pub lon: f64,
}

/// A kind of signal: the SNR it needs (dB in 1 Hz) for the reliability.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Mode {
    pub name: &'static str,
    pub snr_db: f64,
}

/// The modes forecast: SSB speech (about 13 dB in 2.7 kHz), CW (about
/// 0 dB in 500 Hz) and FT8 (its decoding limit, -21 dB in 2.5 kHz).
pub const MODES: [Mode; 3] = [
    Mode {
        name: "SSB",
        snr_db: 48.0,
    },
    Mode {
        name: "CW",
        snr_db: 27.0,
    },
    Mode {
        name: "FT8",
        snr_db: 13.0,
    },
];

/// What to forecast.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub year: i32,
    pub month: u32,
    /// Day of the month (Truecast only).
    pub day: u32,
    pub ssn: f64,
    pub engine: Engine,
    /// The receiver.
    pub lat: f64,
    pub lon: f64,
    pub bands_mhz: Vec<f64>,
    /// Transmit power (W), isotropic antennas at both ends.
    pub power_w: f64,
    /// Man-made noise at 3 MHz at the receiver (dBW in 1 Hz).
    pub noise_dbw: f64,
}

/// One path's forecast; hours are UTC 0 to 23, bands as asked.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PathForecast {
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    /// Great-circle distance (km) and the bearing from the receiver.
    pub km: f64,
    pub bearing: f64,
    /// MHz per hour (none where the engine gives none).
    pub muf: Vec<Option<f64>>,
    pub fot: Vec<Option<f64>>,
    /// Per mode ([`MODES`]): the LUF per hour, and the reliability (0 to
    /// 1) per `[band][hour]`.
    pub luf: Vec<Vec<Option<f64>>>,
    pub rel: Vec<Vec<Vec<Option<f64>>>>,
    /// `[band][hour]`: the median SNR (dB in 1 Hz).
    pub snr: Vec<Vec<Option<f64>>>,
    /// Set when the engine failed for this path (the rest is empty).
    pub error: Option<String>,
}

/// The sunspot number of `year`-`month` from the table built into hfcast,
/// held at the table's ends outside it.
pub fn builtin_ssn(year: i32, month: u32) -> f64 {
    hfcast::wspr::smoothed_ssn_clamped(&format!("{year:04}-{month:02}"))
}

/// NOAA SWPC's predicted solar cycle
/// (`json/solar-cycle/predicted-solar-cycle.json`): month ("2026-11") to
/// the predicted smoothed sunspot number.
pub fn parse_noaa_ssn(body: &[u8]) -> Result<BTreeMap<String, f64>, String> {
    let v: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| format!("sunspot table: {e}"))?;
    let rows = v.as_array().ok_or("sunspot table: not a list")?;
    let mut out = BTreeMap::new();
    for r in rows {
        let (Some(m), Some(n)) = (
            r.get("time-tag").and_then(|t| t.as_str()),
            r.get("predicted_ssn").and_then(|n| n.as_f64()),
        ) else {
            continue;
        };
        let ok = m.len() == 7 && m.as_bytes()[4] == b'-' && (0.0..=500.0).contains(&n);
        if ok {
            out.insert(m.to_string(), n);
        }
    }
    if out.is_empty() {
        return Err("sunspot table: no months".into());
    }
    Ok(out)
}

/// The sunspot number for `year`-`month`: the configured one, else the
/// fetched table's (if it has the month), else the built-in.
pub fn pick_ssn(
    fixed: Option<f64>,
    fetched: Option<(&BTreeMap<String, f64>, u64)>,
    year: i32,
    month: u32,
) -> (f64, SsnSource) {
    if let Some(n) = fixed {
        return (n, SsnSource::Config);
    }
    if let Some((t, at)) = fetched
        && let Some(&n) = t.get(&format!("{year:04}-{month:02}"))
    {
        return (n, SsnSource::Noaa { fetched: at });
    }
    (builtin_ssn(year, month), SsnSource::Builtin)
}

/// The planetary K index (`products/noaa-planetary-k-index.json`): the
/// latest 3-hour value, its time ("2026-11-17T06:00:00"), and the
/// highest of the last 24 hours (8 values).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Kp {
    pub time: String,
    pub kp: f64,
    pub max24: f64,
}

pub fn parse_kp(body: &[u8]) -> Result<Kp, String> {
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|e| format!("Kp: {e}"))?;
    let rows = v.as_array().ok_or("Kp: not a list")?;
    let vals: Vec<(String, f64)> = rows
        .iter()
        .filter_map(|r| {
            Some((
                r.get("time_tag")?.as_str()?.to_string(),
                r.get("Kp")?.as_f64().filter(|k| (0.0..=9.0).contains(k))?,
            ))
        })
        .collect();
    let (time, kp) = vals.last().cloned().ok_or("Kp: no values")?;
    let max24 = vals[vals.len().saturating_sub(8)..]
        .iter()
        .map(|v| v.1)
        .fold(0.0, f64::max);
    Ok(Kp { time, kp, max24 })
}

/// Great-circle distance (km) and initial bearing (degrees) from the
/// first point to the second.
pub fn distance_bearing(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> (f64, f64) {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dl = (lon2 - lon1).to_radians();
    let a = ((p2 - p1) / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    let km = 2.0 * 6371.0 * a.sqrt().min(1.0).asin();
    let y = dl.sin() * p2.cos();
    let x = p1.cos() * p2.sin() - p1.sin() * p2.cos() * dl.cos();
    (km, (y.atan2(x).to_degrees() + 360.0) % 360.0)
}

/// The day index Truecast runs at: the month's number with hfcast's
/// fitted correction for the day of the year (not below zero, where the
/// engine needs more than the built-in maps).
pub fn daily_index(ssn: f64, month: u32, day: u32) -> f64 {
    (ssn + hfcast::truecast::api::offline_anomaly(month, day)).max(0.0)
}

/// The request hfcast's service takes for the path from `t` to the
/// receiver.
fn engine_request(r: &Request, t: &Target, snr_db: f64) -> String {
    let mut q = serde_json::json!({
        "itshfbc": "<embedded>",
        "fromLabel": "", "fromLat": t.lat, "fromLon": t.lon,
        "toLabel": "", "toLat": r.lat, "toLon": r.lon,
        "year": r.year, "month": r.month,
        "watts": r.power_w,
        "requiredSnrDb": snr_db,
        "noiseDbw": r.noise_dbw,
        "bands": r.bands_mhz,
    });
    match r.engine {
        Engine::Voacap => q["ssn"] = r.ssn.round().into(),
        Engine::Truecast => {
            q["engine"] = "truecast".into();
            q["essn"] = daily_index(r.ssn, r.month, r.day).round().into();
            q["day"] = r.day.into();
        }
    }
    q.to_string()
}

fn hours(v: &serde_json::Value, key: &str) -> Vec<Option<f64>> {
    let a = v.get(key).and_then(|a| a.as_array());
    (0..24)
        .map(|h| a.and_then(|a| a.get(h)).and_then(|x| x.as_f64()))
        .collect()
}

/// One engine run's answer.
struct Answer {
    muf: Vec<Option<f64>>,
    luf: Vec<Option<f64>>,
    fot: Vec<Option<f64>>,
    rel: Vec<Vec<Option<f64>>>,
    snr: Vec<Vec<Option<f64>>>,
}

/// Reads hfcast's answer.
fn read_answer(bands: &[f64], out: &str) -> Result<Answer, String> {
    let v: serde_json::Value = serde_json::from_str(out).map_err(|e| e.to_string())?;
    let cells = v
        .get("cells")
        .and_then(|c| c.as_array())
        .ok_or("no cells")?;
    let mut rel = vec![vec![None; 24]; bands.len()];
    let mut snr = vec![vec![None; 24]; bands.len()];
    for c in cells {
        let (Some(f), Some(h)) = (
            c.get("freqMhz").and_then(|x| x.as_f64()),
            c.get("hour").and_then(|x| x.as_u64()),
        ) else {
            continue;
        };
        // the engine has the bands to 0.01 MHz
        let Some(b) = bands.iter().position(|&x| (x - f).abs() < 0.006) else {
            continue;
        };
        let h = h as usize % 24;
        rel[b][h] = c.get("reliability").and_then(|x| x.as_f64());
        snr[b][h] = c.get("snr").and_then(|x| x.as_f64());
    }
    Ok(Answer {
        muf: hours(&v, "mufByHour"),
        luf: hours(&v, "lufByHour"),
        fot: hours(&v, "fotByHour"),
        rel,
        snr,
    })
}

/// One engine run, any panic in it caught.
fn run(q: &str, bands: &[f64]) -> Result<Answer, String> {
    match catch_unwind(AssertUnwindSafe(|| hfcast::service::run(q))) {
        Ok(Ok(out)) => read_answer(bands, &out),
        Ok(Err(e)) => Err(e),
        Err(p) => Err(format!(
            "the engine failed: {}",
            p.downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| p.downcast_ref::<&str>().copied())
                .unwrap_or("panic")
        )),
    }
}

/// The forecast for the path from `t` to the receiver, one engine run
/// per mode. Never panics: an engine failure is the path's `error`.
pub fn path(r: &Request, t: &Target) -> PathForecast {
    let (km, bearing) = distance_bearing(r.lat, r.lon, t.lat, t.lon);
    let mut p = PathForecast {
        name: t.name.clone(),
        lat: t.lat,
        lon: t.lon,
        km,
        bearing,
        muf: Vec::new(),
        fot: Vec::new(),
        luf: Vec::new(),
        rel: Vec::new(),
        snr: Vec::new(),
        error: None,
    };
    for m in MODES {
        match run(&engine_request(r, t, m.snr_db), &r.bands_mhz) {
            Ok(a) => {
                if p.muf.is_empty() {
                    (p.muf, p.fot, p.snr) = (a.muf, a.fot, a.snr);
                }
                p.luf.push(a.luf);
                p.rel.push(a.rel);
            }
            Err(e) => {
                return PathForecast {
                    muf: Vec::new(),
                    fot: Vec::new(),
                    luf: Vec::new(),
                    rel: Vec::new(),
                    snr: Vec::new(),
                    error: Some(e),
                    ..p
                };
            }
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> Request {
        Request {
            year: 2026,
            month: 11,
            day: 17,
            ssn: 80.0,
            engine: Engine::Voacap,
            lat: 42.5,
            lon: -71.0,
            bands_mhz: vec![3.6, 7.1, 14.1, 21.1, 28.2],
            power_w: 100.0,
            noise_dbw: -150.0,
        }
    }

    #[test]
    fn the_noaa_table_and_the_choice_of_sunspot_number() {
        let body = br#"[{"time-tag": "2026-11", "predicted_ssn": 79.5, "high_ssn": 90.1},
                        {"time-tag": "2026-12", "predicted_ssn": 77.0},
                        {"time-tag": "bad", "predicted_ssn": 1.0},
                        {"time-tag": "2027-01"}]"#;
        let t = parse_noaa_ssn(body).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t["2026-11"], 79.5);
        assert!(parse_noaa_ssn(b"[]").is_err());
        assert!(parse_noaa_ssn(b"{").is_err());

        assert_eq!(
            pick_ssn(Some(42.0), Some((&t, 5)), 2026, 11),
            (42.0, SsnSource::Config)
        );
        assert_eq!(
            pick_ssn(None, Some((&t, 5)), 2026, 12),
            (77.0, SsnSource::Noaa { fetched: 5 })
        );
        // a month the fetched table lacks: the built-in one
        let (n, src) = pick_ssn(None, Some((&t, 5)), 2029, 6);
        assert_eq!(src, SsnSource::Builtin);
        assert_eq!(n, builtin_ssn(2029, 6));
        // held at the table's ends
        assert_eq!(builtin_ssn(2050, 1), builtin_ssn(2030, 12));
        assert!((0.0..300.0).contains(&builtin_ssn(2026, 11)));
    }

    #[test]
    fn kp_is_the_latest_and_the_days_highest() {
        let body = br#"[{"time_tag":"2026-11-16T00:00:00","Kp":6.0},
            {"time_tag":"2026-11-16T03:00:00","Kp":2.0},{"time_tag":"2026-11-16T06:00:00","Kp":4.33},
            {"time_tag":"2026-11-16T09:00:00","Kp":1.0},{"time_tag":"2026-11-16T12:00:00","Kp":1.0},
            {"time_tag":"2026-11-16T15:00:00","Kp":1.0},{"time_tag":"2026-11-16T18:00:00","Kp":1.0},
            {"time_tag":"2026-11-16T21:00:00","Kp":1.0},{"time_tag":"2026-11-17T00:00:00","Kp":0.67}]"#;
        let k = parse_kp(body).unwrap();
        assert_eq!(k.time, "2026-11-17T00:00:00");
        assert_eq!(k.kp, 0.67);
        // the 6.0 is more than 24 hours old
        assert_eq!(k.max24, 4.33);
        assert!(parse_kp(b"[]").is_err());
    }

    #[test]
    fn distances_and_bearings() {
        let (km, b) = distance_bearing(0.0, 0.0, 0.0, 90.0);
        assert!((km - 10007.5).abs() < 1.0, "{km}");
        assert!((b - 90.0).abs() < 1e-6);
        let (_, b) = distance_bearing(10.0, 0.0, 50.0, 0.0);
        assert!(b.abs() < 1e-6 || (b - 360.0).abs() < 1e-6);
    }

    #[test]
    fn a_forecast_has_the_hours_and_bands_asked() {
        let r = req();
        let t = Target {
            name: "Europe".into(),
            lat: 50.0,
            lon: 10.0,
        };
        let p = path(&r, &t);
        assert_eq!(p.error, None);
        assert!((5000.0..7000.0).contains(&p.km), "{}", p.km);
        assert_eq!(p.muf.len(), 24);
        assert!(
            p.muf
                .iter()
                .all(|m| m.is_some_and(|m| (2.0..60.0).contains(&m)))
        );
        assert_eq!(p.rel.len(), MODES.len());
        assert_eq!(p.luf.len(), MODES.len());
        assert_eq!(p.snr.len(), 5);
        for m in &p.rel {
            assert_eq!(m.len(), 5);
            for b in m {
                assert_eq!(b.len(), 24);
                assert!(
                    b.iter()
                        .all(|r| r.is_some_and(|r| (0.0..=1.0).contains(&r)))
                );
            }
        }
        // FT8 on 20 m gets through some hour; 80 m isn't open all day;
        // a mode needing less SNR is never less reliable
        let ft8 = MODES.len() - 1;
        assert!(p.rel[ft8][2].iter().flatten().any(|&r| r > 0.5));
        assert!(p.rel[ft8][0].iter().flatten().any(|&r| r < 0.5));
        for b in 0..5 {
            for h in 0..24 {
                assert!(p.rel[ft8][b][h] >= p.rel[0][b][h]);
            }
        }
        // the same day with Truecast: an answer, close to VOACAP's
        let tc = path(
            &Request {
                engine: Engine::Truecast,
                ..r
            },
            &t,
        );
        assert_eq!(tc.error, None);
        let mid = |p: &PathForecast| p.muf[12].unwrap();
        assert!((mid(&tc) - mid(&p)).abs() < 5.0);
    }

    #[test]
    fn an_engine_error_is_the_paths_error() {
        let r = Request { month: 13, ..req() };
        let t = Target {
            name: "x".into(),
            lat: 0.0,
            lon: 0.0,
        };
        let p = path(&r, &t);
        assert!(p.error.is_some());
        assert!(p.muf.is_empty());
    }
}
