// SPDX-License-Identifier: GPL-3.0-or-later
//! The card's boot loader from the admin page (web888-ng-os from v0.10.0,
//! `web888-bootloader`; os `docs/updates.md`): RAUC updates the root
//! slots, each of which ships its own BOOT.BIN and u-boot.img; writing
//! them to the card's FAT is the owner's call, since the boot ROM has no
//! second copy to fall back to.
//!
//! * `GET /api/admin/bootloader`: `{"available", "state": "current" |
//!   "update" | "unsupported", "reason", "blocked", ...}`;
//! * `POST /api/admin/bootloader/install`: write it now; the answer has
//!   the command's output, an error if it failed.
//!
//! Without the command (a development machine, an older image) the page
//! says so.

use crate::admin::{AdminOnly, Fail};
use axum::Router;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::session::Shared;

/// The image's command.
pub const COMMAND: &str = "/usr/sbin/web888-bootloader";

/// One install at a time.
static TURN: Mutex<()> = Mutex::const_new(());

pub fn routes() -> Router<Arc<Shared>> {
    Router::new()
        .route("/api/admin/bootloader", get(status))
        .route("/api/admin/bootloader/install", post(install))
}

/// `key=value` lines as a JSON object (the command's status).
pub fn parse_status(out: &str) -> Map<String, Value> {
    out.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), Value::String(v.trim().to_string())))
        .collect()
}

/// The command's status, or why there is none.
pub fn read_status(cmd: &Path) -> Value {
    if !cmd.is_file() {
        return json!({
            "available": false,
            "reason": "this system can't update the card's boot loader (web888-ng-os v0.10.0 or later on its own boot loader)",
        });
    }
    match Command::new(cmd).arg("status").output() {
        Ok(o) if o.status.success() => {
            let mut m = parse_status(&String::from_utf8_lossy(&o.stdout));
            m.insert("available".into(), json!(true));
            Value::Object(m)
        }
        Ok(o) => json!({
            "available": false,
            "reason": String::from_utf8_lossy(&o.stderr).trim().to_string(),
        }),
        Err(e) => json!({ "available": false, "reason": format!("{}: {e}", cmd.display()) }),
    }
}

/// Run `cmd install`: its output, or its error with the output.
pub fn run_install(cmd: &Path) -> Result<String, String> {
    let o = Command::new(cmd)
        .arg("install")
        .output()
        .map_err(|e| format!("{}: {e}", cmd.display()))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
    .trim()
    .to_string();
    if o.status.success() {
        Ok(text)
    } else {
        Err(text)
    }
}

async fn status(_: AdminOnly) -> Response {
    let v = tokio::task::spawn_blocking(|| read_status(Path::new(COMMAND)))
        .await
        .expect("boot loader status");
    Json(v).into_response()
}

async fn install(_: AdminOnly) -> Response {
    let Ok(_turn) = TURN.try_lock() else {
        return Fail::new(StatusCode::CONFLICT, "already writing the boot loader").into_response();
    };
    let cmd = PathBuf::from(COMMAND);
    if !cmd.is_file() {
        return Fail::new(
            StatusCode::NOT_IMPLEMENTED,
            "this system can't update the card's boot loader",
        )
        .into_response();
    }
    let res = tokio::task::spawn_blocking(move || run_install(&cmd))
        .await
        .expect("boot loader install");
    match res {
        Ok(out) => Json(json!({ "output": out })).into_response(),
        Err(out) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": out.lines().last().unwrap_or("failed"), "output": out })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn status_lines() {
        let m = parse_status("state=update\nblocked=not good yet (tries 2)\nslot=A\n");
        assert_eq!(m["state"], "update");
        assert_eq!(m["blocked"], "not good yet (tries 2)");
        assert_eq!(m["slot"], "A");
    }

    #[test]
    fn without_the_command_and_with_a_fake() {
        let d = std::env::temp_dir().join(format!("web888-bl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let cmd = d.join("web888-bootloader");
        assert_eq!(read_status(&cmd)["available"], false);
        std::fs::write(
            &cmd,
            "#!/bin/sh\ncase $1 in\nstatus) echo state=update ;;\ninstall) echo web888-bootloader: not written: no >&2; exit 1 ;;\nesac\n",
        )
        .unwrap();
        std::fs::set_permissions(&cmd, std::fs::Permissions::from_mode(0o755)).unwrap();
        let s = read_status(&cmd);
        assert_eq!(
            (s["available"].as_bool(), s["state"].as_str()),
            (Some(true), Some("update"))
        );
        assert!(run_install(&cmd).unwrap_err().contains("not written: no"));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
