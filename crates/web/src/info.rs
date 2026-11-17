// SPDX-License-Identifier: GPL-3.0-or-later
//! The receiver's info page for listeners (`doc/m2-web-ui.md` §12):
//!
//! * `GET /api/info`: the name and location (`[kiwi] name`, `location`),
//!   the station's callsign, locator and antenna (`[station]`), the
//!   owner's words (`[info] description`, `contact`), whether there is a
//!   photo, what the Privacy section says (`privacy`: what this receiver
//!   keeps about listeners and the outside services it talks to, from
//!   the settings; `[info] privacy`), whether to show the reception
//!   notice, and the rules a visitor meets (receivers, the per-address,
//!   session, idle and daily limits, receivers open without the password);
//! * `GET /api/info/photo`: the photo;
//! * `GET /api/info/licences`: the third-party libraries built in, with
//!   their licences (`third-party.json`, from `scripts/third-party.py`);
//! * `PUT` / `DELETE /api/admin/info/photo`: the admin sets (JPEG, PNG or
//!   WebP, at most [`PHOTO_MAX`] bytes) or removes it. It is kept next to
//!   the configuration file as `info-photo.jpg` (`.png`, `.webp`).
//!
//! With a listener password and no open receivers, the public ones need a
//! login (as `/ws`).

use crate::admin::{self, AdminOnly};
use crate::session::Shared;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, put};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

/// Largest photo (bytes).
pub const PHOTO_MAX: usize = 3 << 20;

/// The photo's formats: file extension, content type, magic bytes.
const FORMATS: [(&str, &str); 3] = [
    ("jpg", "image/jpeg"),
    ("png", "image/png"),
    ("webp", "image/webp"),
];

pub fn routes() -> Router<Arc<Shared>> {
    Router::new()
        .route("/api/info", get(info))
        .route("/api/info/photo", get(photo))
        .route("/api/info/licences", get(licences))
        .route(
            "/api/admin/info/photo",
            put(photo_put)
                .delete(photo_delete)
                .layer(DefaultBodyLimit::max(PHOTO_MAX + 1024)),
        )
}

/// The format of an image by its first bytes: (extension, content type).
pub fn sniff(b: &[u8]) -> Option<(&'static str, &'static str)> {
    let i = if b.starts_with(&[0xff, 0xd8, 0xff]) {
        0
    } else if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        1
    } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        2
    } else {
        return None;
    };
    Some(FORMATS[i])
}

/// Where the photo is kept (`ext`), next to the configuration file.
fn photo_path(sh: &Shared, ext: &str) -> Option<PathBuf> {
    Some(
        sh.admin
            .hooks
            .config_path
            .as_deref()?
            .parent()?
            .join(format!("info-photo.{ext}")),
    )
}

/// The photo's file and content type, if there is one.
fn find_photo(sh: &Shared) -> Option<(PathBuf, &'static str)> {
    FORMATS
        .iter()
        .find_map(|&(ext, ct)| photo_path(sh, ext).filter(|p| p.is_file()).map(|p| (p, ct)))
}

async fn info(State(sh): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    if !admin::may_listen(&sh, &headers) {
        return (StatusCode::UNAUTHORIZED, "login required").into_response();
    }
    let c = sh.live.borrow().clone();
    let l = &c.limits;
    let password = c.auth.listener_hash.is_some();
    Json(json!({
        "name": c.kiwi.name,
        "location": c.kiwi.location,
        "callsign": c.station.callsign,
        "locator": c.station.locator,
        "antenna": c.station.antenna,
        "description": c.info.description,
        "contact": c.info.contact,
        "photo": find_photo(&sh).is_some(),
        "server": concat!("web888d ", env!("CARGO_PKG_VERSION")),
        "privacy": privacy(&c),
        "reception_notice": c.info.reception_notice,
        "rules": {
            "receivers": sh.channels.channels(),
            "password": password,
            "open_receivers": if password { l.open_channels } else { sh.channels.channels() },
            "per_ip": l.per_ip,
            "session_minutes": l.session_minutes,
            "idle_minutes": l.idle_minutes,
            "daily_minutes": l.daily_minutes,
        },
    }))
    .into_response()
}

/// The libraries in this build (Rust crates, the UI's packages).
const THIRD_PARTY: &str = include_str!("third-party.json");

async fn licences() -> Response {
    ([(header::CONTENT_TYPE, "application/json")], THIRD_PARTY).into_response()
}

/// What the Privacy section says, from the settings: what this receiver
/// keeps about listeners, the outside services it talks to (none of them
/// gets anything about listeners), and the owner's own words.
fn privacy(c: &web888_config::Config) -> serde_json::Value {
    let l = &c.limits;
    let mut services: Vec<serde_json::Value> = Vec::new();
    let mut add = |name: &str, what: &str| services.push(json!({ "name": name, "what": what }));
    if c.propagation.enabled && c.propagation.fetch {
        add(
            "NOAA SWPC",
            "sunspot numbers and the K index, for the propagation forecast",
        );
    }
    if c.dxspots.enabled {
        add("Spothole", "DX spots, shown on the spectrum");
    }
    if c.report.pskreporter {
        add(
            "PSKReporter",
            "the stations the skimmers decode, with this receiver's callsign and locator",
        );
    }
    if c.report.wsprnet {
        add(
            "WSPRnet",
            "the WSPR stations the skimmers decode, with this receiver's callsign and locator",
        );
    }
    if c.update.enabled {
        add("GitHub", "the list of software releases");
    }
    if !c.syslog.server.is_empty() || !c.mqtt.server.is_empty() {
        add(
            "the owner's log servers",
            "what the skimmers decode and do, and ADC overloads",
        );
    }
    json!({
        "per_ip": l.per_ip > 0,
        "daily": l.daily_minutes > 0,
        "blocked": !l.blocked.is_empty(),
        "proxy": !c.web.trusted_proxies.is_empty(),
        "contact": c.info.contact,
        "text": c.info.privacy,
        "services": services,
    })
}

async fn photo(State(sh): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    if !admin::may_listen(&sh, &headers) {
        return (StatusCode::UNAUTHORIZED, "login required").into_response();
    }
    let Some((path, ct)) = find_photo(&sh) else {
        return (StatusCode::NOT_FOUND, "no photo").into_response();
    };
    match tokio::fs::read(&path).await {
        Ok(b) => ([(header::CONTENT_TYPE, ct)], b).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

fn fail(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(json!({ "error": msg.into() }))).into_response()
}

async fn photo_put(_: AdminOnly, State(sh): State<Arc<Shared>>, body: Bytes) -> Response {
    if body.len() > PHOTO_MAX {
        return fail(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("the photo is over {} MB", PHOTO_MAX >> 20),
        );
    }
    let Some((ext, _)) = sniff(&body) else {
        return fail(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "a JPEG, PNG or WebP image, please",
        );
    };
    let Some(path) = photo_path(&sh, ext) else {
        return fail(
            StatusCode::NOT_IMPLEMENTED,
            "this server has no config file to keep the photo beside",
        );
    };
    // written aside, then renamed; the other formats' files go
    let tmp = path.with_extension("new");
    if let Err(e) = tokio::fs::write(&tmp, &body)
        .await
        .and(tokio::fs::rename(&tmp, &path).await)
    {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    for &(other, _) in &FORMATS {
        if other != ext
            && let Some(p) = photo_path(&sh, other)
        {
            let _ = tokio::fs::remove_file(p).await;
        }
    }
    Json(json!({ "photo": true, "bytes": body.len() })).into_response()
}

async fn photo_delete(_: AdminOnly, State(sh): State<Arc<Shared>>) -> Response {
    while let Some((p, _)) = find_photo(&sh) {
        if let Err(e) = tokio::fs::remove_file(&p).await {
            return fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
    }
    Json(json!({ "photo": false })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_library_list_is_whole() {
        let v: serde_json::Value = serde_json::from_str(THIRD_PARTY).unwrap();
        for (kind, at_least) in [("rust", 50), ("npm", 3)] {
            let l = v[kind].as_array().unwrap();
            assert!(l.len() >= at_least, "{kind}: {}", l.len());
            for p in l {
                for k in ["name", "version", "license", "url"] {
                    assert!(p[k].as_str().is_some_and(|s| !s.is_empty()), "{p}: {k}");
                }
            }
        }
        // what the propagation forecasts and the audio rest on
        let names: Vec<&str> = v["rust"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"hfcast") && names.contains(&"opus1"));
    }

    #[test]
    fn photos_are_known_by_their_first_bytes() {
        assert_eq!(
            sniff(&[0xff, 0xd8, 0xff, 0xe0, 0]),
            Some(("jpg", "image/jpeg"))
        );
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n...."), Some(("png", "image/png")));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some(("webp", "image/webp")));
        assert_eq!(sniff(b"GIF89a"), None);
        assert_eq!(sniff(b"<svg"), None);
        assert_eq!(sniff(b""), None);
    }
}
