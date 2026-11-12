// SPDX-License-Identifier: GPL-3.0-or-later
//! Frequency labels and the band ribbon (`web888-markers`):
//!
//! * `GET /api/markers?lo=&hi=&src=eibi,utility,amateur,own,dx&region=1`:
//!   the labels in that range (Hz), EiBi's what is on the air now, `dx`
//!   the live DX spots ([`crate::dxspots`]);
//! * `GET /api/markers/info`: the sources, the station's region, EiBi's
//!   season;
//! * `GET /api/bandplan?region=1`: a region's amateur band plan and the
//!   other services' bands;
//! * `GET` / `PUT /api/admin/markers`: the station's own list as CSV;
//! * `GET /api/map/info`: the receiver's place (locator) for the map;
//! * `GET /api/map/dx`: the live DX spots with places, the latest of each
//!   station on each band.
//!
//! With a listener password, the public ones need a login (as `/ws`).

use crate::admin::{self, AdminOnly};
use crate::session::Shared;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use web888_markers::{Marker, Query as Want, SOURCES, Utc, bandplan, eibi, own};

/// At most this many labels per request.
const LIMIT: usize = 1000;

pub fn routes() -> Router<Arc<Shared>> {
    Router::new()
        .route("/api/markers", get(markers))
        .route("/api/markers/info", get(info))
        .route("/api/bandplan", get(band_plan))
        .route("/api/admin/markers", get(own_get).put(own_put))
        .route("/api/map/info", get(map_info))
        .route("/api/map/dx", get(map_dx))
}

fn allowed(sh: &Shared, headers: &HeaderMap) -> bool {
    sh.live.borrow().auth.listener_hash.is_none() || admin::role(sh, headers).is_some()
}

/// The own list's file: `[markers] file`, a relative path next to the
/// configuration file (none without one).
fn own_path(sh: &Shared) -> Option<PathBuf> {
    let file = sh.live.borrow().markers.file.clone();
    if file.is_absolute() {
        return Some(file);
    }
    let dir = sh.admin.hooks.config_path.as_ref()?.parent()?.to_path_buf();
    Some(dir.join(file))
}

/// The own list as last read: its file, that file's time, the list.
type Cache = Option<(PathBuf, Option<SystemTime>, Arc<Vec<Marker>>)>;
static CACHE: Mutex<Cache> = Mutex::new(None);

/// The own list, read again when its file changes.
fn own_list(sh: &Shared) -> Arc<Vec<Marker>> {
    let Some(path) = own_path(sh) else {
        return Arc::default();
    };
    let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((p, t, list)) = &*c
        && *p == path
        && *t == mtime
    {
        return list.clone();
    }
    let list = Arc::new(own::load(&path).unwrap_or_else(|e| {
        eprintln!("web888d: markers: {e}");
        Vec::new()
    }));
    *c = Some((path, mtime, list.clone()));
    list
}

fn region(sh: &Shared, want: Option<u8>) -> u8 {
    want.unwrap_or_else(|| sh.live.borrow().markers.region)
        .clamp(1, 3)
}

#[derive(Deserialize)]
struct MarkerQuery {
    lo: f64,
    hi: f64,
    /// Comma-separated source ids; default all.
    src: Option<String>,
    region: Option<u8>,
    /// Unix time to filter schedules at (tests); default now.
    t: Option<i64>,
}

async fn markers(
    State(sh): State<Arc<Shared>>,
    headers: HeaderMap,
    Query(q): Query<MarkerQuery>,
) -> Response {
    if !allowed(&sh, &headers) {
        return (StatusCode::UNAUTHORIZED, "login required").into_response();
    }
    if !(q.lo.is_finite() && q.hi.is_finite() && q.lo <= q.hi) {
        return (
            StatusCode::BAD_REQUEST,
            "lo and hi: a frequency range in Hz",
        )
            .into_response();
    }
    let src: Vec<&str> = match &q.src {
        Some(s) => s
            .split(',')
            .map(str::trim)
            .filter(|s| SOURCES.contains(s))
            .collect(),
        None => SOURCES.to_vec(),
    };
    let own = if src.contains(&"own") {
        own_list(&sh)
    } else {
        Arc::default()
    };
    let mut list = web888_markers::query(
        &Want {
            lo: q.lo,
            hi: q.hi,
            sources: &src,
            region: region(&sh, q.region),
            now: q.t.map(Utc::from_unix).unwrap_or_else(Utc::now),
            limit: LIMIT,
        },
        &own,
    );
    if src.contains(&"dx") {
        let max_age = sh.live.borrow().dxspots.max_age_minutes.clamp(1, 60) as f64 * 60.0;
        let now = q.t.map_or_else(
            || {
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map_or(0.0, |d| d.as_secs_f64())
            },
            |t| t as f64,
        );
        list.extend(crate::dxspots::global().markers(q.lo, q.hi, now - max_age));
        list.sort_by(|a, b| a.hz.total_cmp(&b.hz));
        list.truncate(LIMIT);
    }
    Json(serde_json::json!({ "markers": list })).into_response()
}

async fn info(State(sh): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    if !allowed(&sh, &headers) {
        return (StatusCode::UNAUTHORIZED, "login required").into_response();
    }
    let mut sources: Vec<serde_json::Value> = web888_markers::counts()
        .into_iter()
        .map(|(id, n)| serde_json::json!({ "id": id, "count": n }))
        .collect();
    sources.push(serde_json::json!({ "id": "own", "count": own_list(&sh).len() }));
    let dx = crate::dxspots::global();
    sources.push(serde_json::json!({
        "id": "dx",
        "count": dx.len(),
        "enabled": sh.live.borrow().dxspots.enabled,
    }));
    Json(serde_json::json!({
        "region": region(&sh, None),
        "eibi_season": eibi::SEASON,
        "sources": sources,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct PlanQuery {
    region: Option<u8>,
}

async fn band_plan(
    State(sh): State<Arc<Shared>>,
    headers: HeaderMap,
    Query(q): Query<PlanQuery>,
) -> Response {
    if !allowed(&sh, &headers) {
        return (StatusCode::UNAUTHORIZED, "login required").into_response();
    }
    Json(bandplan::plan(region(&sh, q.region))).into_response()
}

async fn own_get(_: AdminOnly, State(sh): State<Arc<Shared>>) -> Response {
    let csv = own::to_csv(&own_list(&sh));
    (
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"markers.csv\"",
            ),
        ],
        csv,
    )
        .into_response()
}

/// A new own list (CSV, as `own_get` gives it): checked, then written in
/// place of the old one (a copy, then a rename).
async fn own_put(_: AdminOnly, State(sh): State<Arc<Shared>>, body: Bytes) -> Response {
    let Some(path) = own_path(&sh) else {
        return error(
            StatusCode::NOT_IMPLEMENTED,
            "this server has no configuration file to keep the list next to",
        );
    };
    let Ok(text) = std::str::from_utf8(&body) else {
        return error(
            StatusCode::BAD_REQUEST,
            "the file is not UTF-8 text (save the CSV as UTF-8)",
        );
    };
    let list = match own::parse(text) {
        Ok(l) => l,
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    let tmp = path.with_extension("csv.new");
    let saved = std::fs::write(&tmp, own::to_csv(&list)).and_then(|_| std::fs::rename(&tmp, &path));
    if let Err(e) = saved {
        let _ = std::fs::remove_file(&tmp);
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{}: {e}", path.display()),
        );
    }
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    Json(serde_json::json!({ "count": list.len() })).into_response()
}

/// The receiver's locator (gpsd's while it has one, else the
/// configured) and callsign: where the map draws it.
async fn map_info(State(sh): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    if !allowed(&sh, &headers) {
        return (StatusCode::UNAUTHORIZED, "login required").into_response();
    }
    let st = sh.live.borrow().station.clone();
    let gps = sh
        .skim
        .as_ref()
        .and_then(|k| k.gpsd.as_ref())
        .and_then(|g| g.lock().unwrap().locator.clone());
    let locator = gps.clone().unwrap_or(st.locator);
    Json(serde_json::json!({
        "locator": locator,
        "call": st.callsign,
        "gps": gps.is_some(),
    }))
    .into_response()
}

/// At most this many DX spots for the map.
const MAP_DX_MAX: usize = 2000;

async fn map_dx(State(sh): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    if !allowed(&sh, &headers) {
        return (StatusCode::UNAUTHORIZED, "login required").into_response();
    }
    let max_age = sh.live.borrow().dxspots.max_age_minutes.clamp(1, 60) as f64 * 60.0;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64());
    let pos = |p: Option<(f64, f64)>| p.map(|(la, lo)| [la, lo]);
    let spots: Vec<serde_json::Value> = crate::dxspots::global()
        .latest(now - max_age, MAP_DX_MAX)
        .into_iter()
        .map(|s| {
            let m = s.marker();
            serde_json::json!({
                "t": s.time,
                "hz": s.hz,
                "band": s.band,
                "mode": s.mode,
                "tune": m.mode,
                "dx": s.dx,
                "de": s.de,
                "note": m.note,
                "dx_pos": pos(s.dx_pos),
                "de_pos": pos(s.de_pos),
            })
        })
        .collect();
    Json(serde_json::json!({ "spots": spots })).into_response()
}

/// An error answer as the admin API gives them.
fn error(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}
