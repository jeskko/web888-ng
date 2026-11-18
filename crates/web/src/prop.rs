// SPDX-License-Identifier: GPL-3.0-or-later
//! HF propagation forecasts for listeners (`doc/m2-web-ui.md` §13,
//! `[propagation]`): the engine is `web888-prop` (VOACAP through hfcast).
//!
//! A task keeps the forecast for this month (and with Truecast, this day)
//! from each region to the receiver. It is computed again when what it
//! depends on changes: the month or day, the sunspot number, the
//! receiver's locator, the settings.
//!
//! With `fetch` on, NOAA's predicted sunspot numbers are fetched daily
//! and kept in `[web] state_dir` (so a receiver that loses its network
//! keeps the last table), and the planetary K index every 30 minutes
//! (shown only). With it off, or with no network, the sunspot number is
//! the configured one or the table built into the engine.
//!
//! `GET /api/propagation`: the forecast and where its inputs came from
//! (needs a login where `/ws` does).

use crate::admin;
use crate::session::Shared;
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::watch;
use web888_config::{Config, PropEngine, Propagation};
use web888_prop as prop;

/// Largest answers taken (the sunspot table is ~20 kB, the K index ~5 kB).
const MAX_SSN: usize = 512 << 10;
const MAX_KP: usize = 128 << 10;
/// How often: the sunspot table, the K index, and a failed fetch again.
const SSN_EVERY: f64 = 24.0 * 3600.0;
const KP_EVERY: f64 = 1800.0;
const RETRY: f64 = 3600.0;
/// The cached sunspot table, in `state_dir`.
const CACHE: &str = "propagation-ssn.json";

/// What the forecast was made from; a new one is made when it changes.
#[derive(Clone, Debug, PartialEq)]
struct Key {
    req: prop::Request,
    targets: Vec<prop::Target>,
    locator: String,
}

#[derive(Default)]
struct St {
    /// NOAA's table and when it was fetched; when it was last tried.
    ssn: Option<(BTreeMap<String, f64>, u64)>,
    ssn_tried: f64,
    ssn_error: Option<String>,
    kp: Option<(prop::Kp, u64)>,
    kp_tried: f64,
    kp_error: Option<String>,
    /// The forecast (`/api/propagation`), its inputs, and why there is none.
    key: Option<Key>,
    forecast: Option<Value>,
    problem: Option<String>,
    /// The cache was read (once, at the start).
    loaded: bool,
}

fn st() -> &'static Mutex<St> {
    static S: OnceLock<Mutex<St>> = OnceLock::new();
    S.get_or_init(Mutex::default)
}

fn lock() -> std::sync::MutexGuard<'static, St> {
    st().lock().unwrap_or_else(|e| e.into_inner())
}

fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

pub fn routes() -> Router<Arc<Shared>> {
    Router::new().route("/api/propagation", get(forecast))
}

async fn forecast(State(sh): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    if !admin::may_read(&sh, &headers) {
        return (StatusCode::UNAUTHORIZED, "login required").into_response();
    }
    let c = sh.live.borrow().propagation.clone();
    let s = lock();
    let kp =
        s.kp.as_ref()
            .map(|(k, at)| json!({ "time": k.time, "kp": k.kp, "max24": k.max24, "fetched": at }));
    let errors: Vec<String> = [s.ssn_error.clone(), s.kp_error.clone()]
        .into_iter()
        .flatten()
        .collect();
    Json(json!({
        "enabled": c.enabled,
        "fetch": c.fetch,
        "problem": s.problem,
        "fetch_errors": errors,
        "kp": kp,
        "forecast": s.forecast,
    }))
    .into_response()
}

/// The sunspot table cached on disk: `{"fetched": t, "months": {...}}`.
fn read_cache(dir: &Path) -> Option<(BTreeMap<String, f64>, u64)> {
    let b = std::fs::read(dir.join(CACHE)).ok()?;
    let v: Value = serde_json::from_slice(&b).ok()?;
    let at = v.get("fetched")?.as_u64()?;
    let months = serde_json::from_value(v.get("months")?.clone()).ok()?;
    Some((months, at))
}

async fn write_cache(dir: &Path, t: &BTreeMap<String, f64>, at: u64) -> std::io::Result<()> {
    let body = json!({ "fetched": at, "months": t }).to_string();
    tokio::fs::create_dir_all(dir).await?;
    let tmp = dir.join(format!("{CACHE}.new"));
    tokio::fs::write(&tmp, body).await?;
    tokio::fs::rename(&tmp, dir.join(CACHE)).await
}

/// Fetches what is due (`fetch` on).
async fn fetch_due(c: &Propagation, dir: &Path) {
    let now = unix_now();
    let (ssn_due, kp_due) = {
        let s = lock();
        let age = s.ssn.as_ref().map_or(f64::INFINITY, |t| now - t.1 as f64);
        let kp_age = s.kp.as_ref().map_or(f64::INFINITY, |k| now - k.1 as f64);
        (
            age >= SSN_EVERY && now - s.ssn_tried >= RETRY,
            kp_age >= KP_EVERY && now - s.kp_tried >= KP_EVERY.min(RETRY),
        )
    };
    if ssn_due {
        lock().ssn_tried = now;
        let r = crate::dxspots::fetch(&c.ssn_url, &c.ca_file, "application/json", MAX_SSN)
            .await
            .and_then(|b| prop::parse_noaa_ssn(&b));
        match r {
            Ok(t) => {
                let at = now as u64;
                if let Err(e) = write_cache(dir, &t, at).await {
                    eprintln!("propagation: keeping the sunspot table: {e}");
                }
                let mut s = lock();
                s.ssn = Some((t, at));
                s.ssn_error = None;
            }
            Err(e) => {
                eprintln!("propagation: sunspot numbers: {e}");
                lock().ssn_error = Some(format!("sunspot numbers: {e}"));
            }
        }
    }
    if kp_due {
        lock().kp_tried = now;
        let r = crate::dxspots::fetch(&c.kp_url, &c.ca_file, "application/json", MAX_KP)
            .await
            .and_then(|b| prop::parse_kp(&b));
        let mut s = lock();
        match r {
            Ok(k) => {
                s.kp = Some((k, now as u64));
                s.kp_error = None;
            }
            Err(e) => s.kp_error = Some(format!("K index: {e}")),
        }
    }
}

/// The receiver's locator: gpsd's when there is one, else `[station]`'s.
fn locator(c: &Config, gps: Option<&Mutex<web888_skim::gpsd::Status>>) -> String {
    if !c.station.gpsd.is_empty()
        && let Some(l) = gps.and_then(|g| g.lock().ok()?.locator.clone())
    {
        return l;
    }
    c.station.locator.clone()
}

/// What the forecast would be made from now, or why there is none.
fn inputs(c: &Config, loc: &str, now: f64) -> Result<Key, String> {
    let p = &c.propagation;
    let (lat, lon) = crate::kiwi::locator_pos(loc)
        .ok_or("the receiver's location is unknown ([station] locator)")?;
    let bands: Vec<f64> = p
        .bands_mhz
        .iter()
        .copied()
        .filter(|f| (2.0..=30.0).contains(f))
        .take(prop::MAX_BANDS)
        .collect();
    if bands.is_empty() || p.targets.is_empty() {
        return Err("no bands or regions ([propagation] bands_mhz, targets)".into());
    }
    let u = web888_skim::utc::utc(now);
    let engine = match p.engine {
        PropEngine::Voacap => prop::Engine::Voacap,
        PropEngine::Truecast => prop::Engine::Truecast,
    };
    let ssn = {
        let s = lock();
        let fetched = s.ssn.as_ref().filter(|_| p.fetch).map(|(t, at)| (t, *at));
        prop::pick_ssn(
            p.ssn.map(|n| n.clamp(0.0, 300.0)),
            fetched,
            u.year as i32,
            u.month,
        )
        .0
    };
    Ok(Key {
        req: prop::Request {
            year: u.year as i32,
            month: u.month,
            // the month's median doesn't change with the day
            day: if engine == prop::Engine::Truecast {
                u.day
            } else {
                15
            },
            ssn: ssn.round(),
            engine,
            lat,
            lon,
            bands_mhz: bands,
            power_w: p.power_w.clamp(0.1, 1e6),
            noise_dbw: p.noise_dbw.clamp(-170.0, -120.0),
        },
        targets: p
            .targets
            .iter()
            .map(|t| prop::Target {
                name: t.name.clone(),
                lat: t.lat.clamp(-90.0, 90.0),
                lon: t.lon.clamp(-180.0, 180.0),
            })
            .collect(),
        locator: loc.to_string(),
    })
}

/// The forecast for `k` (blocking: ~0.6 s per region on the receiver).
fn compute(k: &Key, source: prop::SsnSource) -> Value {
    let paths: Vec<prop::PathForecast> = k.targets.iter().map(|t| prop::path(&k.req, t)).collect();
    let r = &k.req;
    json!({
        "computed": unix_now() as u64,
        "year": r.year,
        "month": r.month,
        "day": (r.engine == prop::Engine::Truecast).then_some(r.day),
        "ssn": r.ssn,
        "ssn_source": source,
        "engine": r.engine,
        "power_w": r.power_w,
        "noise_dbw": r.noise_dbw,
        "rx": { "locator": k.locator, "lat": r.lat, "lon": r.lon },
        "modes": prop::MODES,
        "bands": r.bands_mhz,
        "paths": paths,
    })
}

/// Keeps the forecast; on the web runtime.
pub async fn run(
    mut live: watch::Receiver<Arc<Config>>,
    gps: Option<Arc<Mutex<web888_skim::gpsd::Status>>>,
) {
    loop {
        let c = live.borrow().clone();
        let dir: PathBuf = c.web.state_dir.clone();
        if !lock().loaded {
            let cached = read_cache(&dir);
            let mut s = lock();
            s.loaded = true;
            s.ssn = cached;
        }
        let p = &c.propagation;
        if !p.enabled {
            let mut s = lock();
            (s.key, s.forecast, s.problem) = (None, None, None);
        } else {
            if p.fetch {
                fetch_due(p, &dir).await;
            }
            let loc = locator(&c, gps.as_deref());
            match inputs(&c, &loc, unix_now()) {
                Err(e) => {
                    let mut s = lock();
                    (s.key, s.forecast, s.problem) = (None, None, Some(e));
                }
                Ok(k) if lock().key.as_ref() != Some(&k) => {
                    let source = {
                        let s = lock();
                        let fetched = s.ssn.as_ref().filter(|_| p.fetch).map(|(t, at)| (t, *at));
                        prop::pick_ssn(p.ssn, fetched, k.req.year, k.req.month).1
                    };
                    let k2 = k.clone();
                    let t0 = std::time::Instant::now();
                    match tokio::task::spawn_blocking(move || compute(&k2, source)).await {
                        Ok(v) => {
                            eprintln!(
                                "propagation: forecast for {}-{:02}, SSN {}, {} regions ({:.1} s)",
                                k.req.year,
                                k.req.month,
                                k.req.ssn,
                                k.targets.len(),
                                t0.elapsed().as_secs_f64()
                            );
                            let mut s = lock();
                            (s.key, s.forecast, s.problem) = (Some(k), Some(v), None);
                        }
                        Err(e) => lock().problem = Some(format!("the forecast failed: {e}")),
                    }
                }
                Ok(_) => {}
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(60)) => {}
            r = live.changed() => if r.is_err() { return },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_follow_the_settings_and_the_date() {
        let mut c = Config::default();
        // 2026-11-17 12:00 UTC
        let now = 1_794_916_800.0;
        assert!(inputs(&c, "", now).is_err());
        let k = inputs(&c, "FN42aa", now).unwrap();
        assert_eq!((k.req.year, k.req.month, k.req.day), (2026, 11, 15));
        assert_eq!(k.req.bands_mhz.len(), 9);
        assert_eq!(k.targets.len(), 7);
        assert!((k.req.lat - 42.02).abs() < 0.1 && (k.req.lon + 71.96).abs() < 0.1);
        // a fixed sunspot number; the day with Truecast
        c.propagation.ssn = Some(42.4);
        c.propagation.engine = PropEngine::Truecast;
        let k = inputs(&c, "FN42aa", now).unwrap();
        assert_eq!(k.req.ssn, 42.0);
        assert_eq!(k.req.day, 17);
        // bands outside 2 to 30 MHz are left out
        c.propagation.bands_mhz = vec![1.8, 7.1, 50.1];
        assert_eq!(inputs(&c, "FN42", now).unwrap().req.bands_mhz, vec![7.1]);
        c.propagation.bands_mhz.clear();
        assert!(inputs(&c, "FN42", now).is_err());
    }

    #[test]
    fn the_cache_reads_back() {
        let dir = std::env::temp_dir().join(format!("web888-prop-{}", std::process::id()));
        let t: BTreeMap<String, f64> = [("2026-11".to_string(), 79.5)].into();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(write_cache(&dir, &t, 1234))
            .unwrap();
        assert_eq!(read_cache(&dir), Some((t, 1234)));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(read_cache(&dir), None);
    }
}
