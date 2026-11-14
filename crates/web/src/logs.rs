// SPDX-License-Identifier: GPL-3.0-or-later
//! The logs on the admin page: the last lines of the system's journal
//! (web888d's, this boot's whole, the kernel's) and the image's
//! `web888-setup.log` (what `web888.txt` set at boot). Also in safe mode,
//! where they say why the radio didn't start.
//!
//! * `GET /api/admin/logs?source=web888d&lines=300`: `{"source", "lines":
//!   [...], "sources": {name: available}}`. Sources: `web888d`, `boot`,
//!   `kernel` (`journalctl`), `setup` (the file).
//!
//! Without a journal (a development machine without systemd) or the file,
//! that source says so.

use crate::admin::AdminOnly;
use crate::session::Shared;
use axum::Router;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use serde::Deserialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

/// Lines by default, and at most.
pub const LINES: usize = 300;
pub const LINES_MAX: usize = 5000;
/// A line longer than this is cut (a stray binary dump).
const LINE_MAX: usize = 2000;

pub fn routes() -> Router<Arc<Shared>> {
    Router::new().route("/api/admin/logs", get(logs))
}

/// Where the logs are (tests point them elsewhere).
#[derive(Clone, Debug)]
pub struct Sys {
    pub journalctl: PathBuf,
    /// The journal's runtime directory (journald runs).
    pub journal: PathBuf,
    pub setup_log: PathBuf,
}

impl Default for Sys {
    fn default() -> Self {
        Sys {
            journalctl: "/usr/bin/journalctl".into(),
            journal: "/run/systemd/journal".into(),
            // the FAT, next to web888.txt (web888-ng-os)
            setup_log: "/boot/web888-setup.log".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Source {
    Web888d,
    Boot,
    Kernel,
    Setup,
}

impl Source {
    pub const ALL: [Source; 4] = [Source::Web888d, Source::Boot, Source::Kernel, Source::Setup];

    pub fn name(self) -> &'static str {
        match self {
            Source::Web888d => "web888d",
            Source::Boot => "boot",
            Source::Kernel => "kernel",
            Source::Setup => "setup",
        }
    }

    fn parse(s: &str) -> Option<Source> {
        Source::ALL.into_iter().find(|x| x.name() == s)
    }
}

impl Sys {
    fn has_journal(&self) -> bool {
        self.journalctl.is_file() && self.journal.is_dir()
    }

    pub fn available(&self, src: Source) -> bool {
        match src {
            Source::Setup => self.setup_log.is_file(),
            _ => self.has_journal(),
        }
    }

    /// The last `n` lines of `src`, oldest first.
    pub fn read(&self, src: Source, n: usize) -> Result<Vec<String>, String> {
        let n = n.clamp(1, LINES_MAX);
        if !self.available(src) {
            return Err(match src {
                Source::Setup => format!(
                    "no {} (it is written at boot on the web888-ng-os card image)",
                    self.setup_log.display()
                ),
                _ => "no system journal here (journalctl and journald, on the web888-ng-os image)"
                    .into(),
            });
        }
        let text = match src {
            Source::Setup => std::fs::read(&self.setup_log)
                .map_err(|e| format!("{}: {e}", self.setup_log.display()))?,
            _ => journal(&self.journalctl, src, n)?,
        };
        Ok(last_lines(&String::from_utf8_lossy(&text), n))
    }
}

/// `journalctl`'s output for `src`: the last `n` entries, ISO times.
fn journal(bin: &Path, src: Source, n: usize) -> Result<Vec<u8>, String> {
    let mut c = Command::new(bin);
    c.args(["--no-pager", "-q", "-o", "short-iso", "-n"])
        .arg(n.to_string());
    match src {
        // the normal service and the safe-mode one (whichever ran)
        Source::Web888d => c.args(["-u", "web888d.service", "-u", "web888d-safe.service"]),
        Source::Boot => c.arg("-b"),
        Source::Kernel => c.args(["-k", "-b"]),
        Source::Setup => unreachable!("a file"),
    };
    let out = c.output().map_err(|e| format!("{}: {e}", bin.display()))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "journalctl: {}",
            err.lines().next().unwrap_or("failed").trim()
        ));
    }
    Ok(out.stdout)
}

/// The last `n` lines of `text`, each cut to [`LINE_MAX`] bytes.
fn last_lines(text: &str, n: usize) -> Vec<String> {
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(n)..]
        .iter()
        .map(|l| {
            if l.len() <= LINE_MAX {
                l.to_string()
            } else {
                let mut end = LINE_MAX;
                while !l.is_char_boundary(end) {
                    end -= 1;
                }
                format!("{}…", &l[..end])
            }
        })
        .collect()
}

#[derive(Deserialize)]
struct LogQuery {
    source: Option<String>,
    lines: Option<usize>,
}

async fn logs(_: AdminOnly, Query(q): Query<LogQuery>) -> Response {
    let src = match q.source.as_deref() {
        None => Source::Web888d,
        Some(s) => match Source::parse(s) {
            Some(s) => s,
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": format!("source: one of web888d, boot, kernel, setup, not {s:?}") })),
                )
                    .into_response();
            }
        },
    };
    let n = q.lines.unwrap_or(LINES);
    let sys = Sys::default();
    let res = tokio::task::spawn_blocking(move || {
        let sources: serde_json::Map<String, serde_json::Value> = Source::ALL
            .into_iter()
            .map(|s| (s.name().to_string(), json!(sys.available(s))))
            .collect();
        (sys.read(src, n), sources)
    })
    .await
    .expect("reading the logs");
    match res {
        (Ok(lines), sources) => Json(json!({
            "source": src.name(),
            "lines": lines,
            "sources": sources,
        }))
        .into_response(),
        (Err(e), sources) => Json(json!({
            "source": src.name(),
            "lines": [],
            "error": e,
            "sources": sources,
        }))
        .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("web888-logs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn last_lines_and_cut() {
        assert_eq!(last_lines("a\nb\nc\n", 2), ["b", "c"]);
        assert_eq!(last_lines("a\n", 5), ["a"]);
        let long = "é".repeat(LINE_MAX);
        let l = &last_lines(&long, 1)[0];
        assert!(l.ends_with('…') && l.len() <= LINE_MAX + '…'.len_utf8());
    }

    #[test]
    fn the_setup_file_and_a_fake_journal() {
        let d = tmp("sys");
        let fake = d.join("journalctl");
        // prints its arguments, one a line
        std::fs::write(&fake, "#!/bin/sh\nfor a in \"$@\"; do echo \"$a\"; done\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let sys = Sys {
            journalctl: fake,
            journal: d.join("journal"),
            setup_log: d.join("web888-setup.log"),
        };
        // neither yet
        assert!(
            sys.read(Source::Setup, 10)
                .unwrap_err()
                .contains("web888-setup.log")
        );
        assert!(
            sys.read(Source::Web888d, 10)
                .unwrap_err()
                .contains("journal")
        );
        std::fs::write(&sys.setup_log, "one\ntwo\nthree\n").unwrap();
        std::fs::create_dir(&sys.journal).unwrap();
        assert_eq!(sys.read(Source::Setup, 2).unwrap(), ["two", "three"]);
        let args = sys.read(Source::Web888d, 7).unwrap();
        assert!(
            args.contains(&"web888d-safe.service".to_string()),
            "{args:?}"
        );
        assert!(args.contains(&"7".to_string()));
        let args = sys.read(Source::Kernel, LINES_MAX + 10).unwrap();
        assert!(args.contains(&"-k".to_string()));
        assert!(args.contains(&LINES_MAX.to_string()), "clamped: {args:?}");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
