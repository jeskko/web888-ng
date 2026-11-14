// SPDX-License-Identifier: GPL-3.0-or-later
//! Settings backup and restore from the admin page:
//!
//! * `GET /api/admin/backup`: one JSON file to download, with the
//!   configuration file as TOML (passwords as their hashes) and the own
//!   frequency labels as CSV;
//! * `POST /api/admin/restore` (that file): both checked first, then
//!   written as the admin page's own saves are; the answer says which
//!   settings apply at once and which need a restart.
//!
//! The OS's own settings (network, tunnels, the SSH key) are the owner's
//! `web888.txt` on the card's FAT, not here.

use crate::admin::{self, AdminOnly, Fail};
use crate::markers;
use crate::session::Shared;
use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use web888_config::Config;
use web888_markers::own;

/// The backup's `format`.
pub const FORMAT: &str = "web888d-settings";
/// Its version: a newer file than this server reads is refused.
pub const VERSION: u32 = 1;

pub fn routes() -> Router<Arc<Shared>> {
    Router::new()
        .route("/api/admin/backup", get(backup))
        .route("/api/admin/restore", post(restore))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Backup {
    pub format: String,
    pub version: u32,
    /// The server that wrote it, and when (Unix seconds).
    pub server: String,
    pub created: u64,
    /// The configuration file (TOML).
    pub config: String,
    /// The own labels (CSV); none without a file.
    pub labels: Option<String>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The file's name: the date (UTC) it was made.
fn file_name(unix: u64) -> String {
    let days = unix / 86_400;
    // civil from days (H. Hinnant)
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("web888d-settings-{y:04}-{m:02}-{d:02}.json")
}

async fn backup(_: AdminOnly, State(sh): State<Arc<Shared>>) -> Response {
    let file = match admin::file_config(&sh) {
        Ok(c) => c,
        Err(r) => return r.into_response(),
    };
    let labels = markers::own_file(&file.markers.file, sh.admin.hooks.config_path.as_deref())
        .and_then(|p| std::fs::read_to_string(p).ok());
    let created = now();
    let b = Backup {
        format: FORMAT.into(),
        version: VERSION,
        server: env!("CARGO_PKG_VERSION").into(),
        created,
        config: file.to_toml(),
        labels,
    };
    let body = serde_json::to_string_pretty(&b).expect("backup serialises");
    (
        [
            (header::CONTENT_TYPE, "application/json".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", file_name(created)),
            ),
        ],
        body,
    )
        .into_response()
}

/// A backup's contents, checked.
pub fn check(b: &Backup) -> Result<(Config, Option<Vec<web888_markers::Marker>>), String> {
    if b.format != FORMAT {
        return Err(format!(
            "not a web888d settings backup (format {:?})",
            b.format
        ));
    }
    if b.version > VERSION {
        return Err(format!(
            "a backup of version {} from web888d {}: this server reads up to version {VERSION}",
            b.version, b.server
        ));
    }
    let c = Config::parse(&b.config).map_err(|e| format!("its configuration: {e}"))?;
    let labels = match &b.labels {
        Some(t) => Some(own::parse(t).map_err(|e| format!("its labels: {e}"))?),
        None => None,
    };
    Ok((c, labels))
}

async fn restore(
    _: AdminOnly,
    State(sh): State<Arc<Shared>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    // JSON only, as the rest of the admin API (no cross-site form posts)
    let json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if !json {
        return Fail::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "the backup goes as application/json",
        )
        .into_response();
    }
    let b: Backup = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => {
            return Fail::new(
                StatusCode::BAD_REQUEST,
                format!("not a web888d settings backup: {e}"),
            )
            .into_response();
        }
    };
    let (c, labels) = match check(&b) {
        Ok(x) => x,
        Err(e) => return Fail::new(StatusCode::BAD_REQUEST, e).into_response(),
    };
    let changes = admin::changes_json(&sh, &c);
    // the labels first: their file is where the restored configuration
    // says
    if let Some(list) = &labels {
        match markers::own_file(&c.markers.file, sh.admin.hooks.config_path.as_deref()) {
            Some(p) => {
                if let Err(e) = markers::write_own(&p, list) {
                    return Fail::new(StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
                }
            }
            None => {
                return Fail::new(
                    StatusCode::NOT_IMPLEMENTED,
                    "this server has no configuration file to keep the labels next to",
                )
                .into_response();
            }
        }
    }
    if let Err(r) = admin::save(&sh, &c) {
        return r.into_response();
    }
    let mut out = changes;
    out["labels"] = serde_json::json!(labels.map(|l| l.len()));
    out["from"] = serde_json::json!({ "server": b.server, "created": b.created });
    Json(out).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_by_date() {
        assert_eq!(file_name(0), "web888d-settings-1970-01-01.json");
        // 2026-11-14 12:00 UTC
        assert_eq!(file_name(1_794_657_600), "web888d-settings-2026-11-14.json");
        assert_eq!(file_name(951_782_400), "web888d-settings-2000-02-29.json");
    }

    fn sample() -> Backup {
        Backup {
            format: FORMAT.into(),
            version: VERSION,
            server: "0.13.0".into(),
            created: 0,
            config: Config::default().to_toml(),
            labels: None,
        }
    }

    #[test]
    fn checks_before_writing() {
        assert!(check(&sample()).is_ok());
        let b = Backup {
            format: "other".into(),
            ..sample()
        };
        assert!(check(&b).unwrap_err().contains("not a web888d"));
        let b = Backup {
            version: VERSION + 1,
            ..sample()
        };
        assert!(check(&b).unwrap_err().contains("reads up to"));
        let b = Backup {
            config: "[radio]\nno_such_key = 1\n".into(),
            ..sample()
        };
        assert!(check(&b).unwrap_err().contains("configuration"));
    }
}
