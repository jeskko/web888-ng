//! Safe mode (`web888d serve --safe`): the web and admin pages without
//! the radio, for a unit whose normal start keeps failing (web888-ng-os
//! `docs/safe-mode.md`). The image starts web888d this way when no root
//! slot has tries left or `web888.txt` says `safe = yes`.
//!
//! * `GET /api/safe`: `{"safe": bool, "reason": "..."}` (anyone: the UI's
//!   banner).
//! * `POST /api/admin/safe/leave`: run the image's `web888-safe leave`
//!   (the slots get their tries back, the unit reboots).

use crate::admin::AdminOnly;
use crate::session::Shared;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// Why (the image's `/run/web888/safe`), and the command that leaves.
#[derive(Clone, Debug)]
pub struct Safe {
    pub reason: String,
    pub leave: Vec<String>,
}

static SAFE: OnceLock<Safe> = OnceLock::new();

/// The reason file's first line, if it has one.
fn read_reason(file: &Path) -> Option<String> {
    std::fs::read_to_string(file)
        .ok()
        .map(|s| s.lines().next().unwrap_or("").trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Called once at start with `--safe`. The reason comes from `file` if
/// it is there (one line), else says the option was given.
pub fn enter(file: &Path) {
    let _ = SAFE.set(Safe {
        reason: read_reason(file).unwrap_or_else(|| "started with --safe".into()),
        leave: vec!["/usr/sbin/web888-safe".into(), "leave".into()],
    });
}

pub fn state() -> Option<&'static Safe> {
    SAFE.get()
}

/// The reason file of the web888-ng-os image.
pub fn default_file() -> PathBuf {
    PathBuf::from("/run/web888/safe")
}

pub fn routes() -> Router<Arc<Shared>> {
    Router::new()
        .route("/api/safe", get(status))
        .route("/api/admin/safe/leave", post(leave))
}

async fn status() -> Response {
    match state() {
        Some(s) => Json(json!({ "safe": true, "reason": s.reason })),
        None => Json(json!({ "safe": false })),
    }
    .into_response()
}

async fn leave(_: AdminOnly) -> Response {
    let Some(s) = state() else {
        return (StatusCode::CONFLICT, "not in safe mode").into_response();
    };
    let cmd = s.leave.clone();
    if !Path::new(&cmd[0]).exists() {
        return (
            StatusCode::NOT_IMPLEMENTED,
            "this system has no web888-safe: restart web888d without --safe",
        )
            .into_response();
    }
    eprintln!("safe mode: leaving, from the admin page");
    // answer first, then go (the command reboots)
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let _ = tokio::task::spawn_blocking(move || Command::new(&cmd[0]).args(&cmd[1..]).status())
            .await;
    });
    Json(json!({})).into_response()
}

#[cfg(test)]
mod tests {
    use super::read_reason;

    #[test]
    fn reason_is_the_files_first_line() {
        let d = std::env::temp_dir().join(format!("web888-safe-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("safe");
        std::fs::write(&f, "  no root slot has tries left \nmore\n").unwrap();
        assert_eq!(
            read_reason(&f).as_deref(),
            Some("no root slot has tries left")
        );
        std::fs::write(&f, "\n").unwrap();
        assert_eq!(read_reason(&f), None);
        assert_eq!(read_reason(&d.join("missing")), None);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
