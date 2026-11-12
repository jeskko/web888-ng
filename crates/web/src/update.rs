// SPDX-License-Identifier: GPL-3.0-or-later
//! OS updates from the admin page (`[update]`), on the web888-ng-os image
//! (RAUC; os `docs/updates.md`):
//!
//! * `GET /api/admin/update`: the running OS version, the root slots
//!   (`rauc status`), the newest release found and the install's state;
//! * `POST /api/admin/update/check`: look for a release now;
//! * `POST /api/admin/update/install` `{"version": "0.9.0"}`: stream that
//!   release's bundle into the other slot (`rauc install URL`; the
//!   receiver keeps running);
//! * `POST /api/admin/update/reboot`: start the new system.
//!
//! The releases are a GitHub repository's (its API's list, the highest
//! version with a `.raucb` bundle: os releases are marked pre-releases).
//! With `check_hours`, web888d also looks by itself; it never installs by
//! itself. RAUC checks the bundle's signature against the image's
//! keyring, so only the project's signed bundles install; U-Boot falls
//! back to the old slot if the new system doesn't come up.
//!
//! Without RAUC (a development machine, the chain-loaded image) the page
//! says so and nothing runs.

use crate::admin::AdminOnly;
use crate::session::Shared;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::watch;
use web888_config::Config;

/// The releases' list, at most this big.
const MAX_LIST: usize = 4 << 20;
/// Lines of `rauc install`'s output kept for the page.
const LOG_LINES: usize = 40;
/// A release's notes, at most this long on the page.
const NOTES_MAX: usize = 4000;

pub fn routes() -> Router<Arc<Shared>> {
    Router::new()
        .route("/api/admin/update", get(status))
        .route("/api/admin/update/check", post(check_now))
        .route("/api/admin/update/install", post(install))
        .route("/api/admin/update/reboot", post(reboot))
}

/// Where the system's parts are (tests point them elsewhere).
#[derive(Clone, Debug)]
pub struct Sys {
    pub rauc: PathBuf,
    /// RAUC's configuration: any of these (where RAUC looks).
    pub system_conf: Vec<PathBuf>,
    pub os_release: PathBuf,
    pub reboot: Vec<String>,
}

impl Default for Sys {
    fn default() -> Self {
        Sys {
            rauc: "/usr/bin/rauc".into(),
            system_conf: vec![
                "/etc/rauc/system.conf".into(),
                "/run/rauc/system.conf".into(),
                "/usr/lib/rauc/system.conf".into(),
            ],
            os_release: "/etc/os-release".into(),
            reboot: vec!["systemctl".into(), "reboot".into()],
        }
    }
}

impl Sys {
    /// Why updates can't run here, if they can't.
    fn missing(&self) -> Option<&'static str> {
        if !self.rauc.exists() || !self.system_conf.iter().any(|p| p.exists()) {
            return Some("this system has no RAUC (updates are for the web888-ng-os card image)");
        }
        None
    }

    /// `VERSION_ID` of os-release.
    fn os_version(&self) -> Option<String> {
        let text = std::fs::read_to_string(&self.os_release).ok()?;
        text.lines().find_map(|l| {
            l.strip_prefix("VERSION_ID=")
                .map(|v| v.trim().trim_matches('"').to_string())
        })
    }
}

/// A release with a bundle.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub name: String,
    pub published: String,
    /// The bundle's download address.
    pub url: String,
    pub size: u64,
    /// The release's page.
    pub page: String,
    pub notes: String,
    pub prerelease: bool,
}

impl Release {
    fn json(&self) -> Value {
        json!({
            "version": self.version,
            "name": self.name,
            "published": self.published,
            "url": self.url,
            "size": self.size,
            "page": self.page,
            "notes": self.notes,
            "prerelease": self.prerelease,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Phase {
    #[default]
    Idle,
    Running,
    Done,
    Failed,
}

#[derive(Clone, Debug, Default)]
pub struct Install {
    pub phase: Phase,
    pub version: String,
    pub percent: u32,
    pub step: String,
    pub started: f64,
    pub ended: f64,
    pub error: String,
    pub log: Vec<String>,
}

#[derive(Default)]
struct St {
    latest: Option<Release>,
    checked: Option<f64>,
    check_error: Option<String>,
    install: Install,
}

pub struct Updater {
    sys: Sys,
    st: Mutex<St>,
}

pub fn global() -> &'static Updater {
    static U: OnceLock<Updater> = OnceLock::new();
    U.get_or_init(|| Updater::new(Sys::default()))
}

fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// "v0.10.0", "0.10.0-rc1": the numbers, then whether it is a final
/// version (one without a suffix is after its suffixed ones).
fn version_key(v: &str) -> (Vec<u64>, bool) {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let (nums, suffix) = match v.find(|c: char| !(c.is_ascii_digit() || c == '.')) {
        Some(k) => (&v[..k], !v[k..].is_empty()),
        None => (v, false),
    };
    let mut n: Vec<u64> = nums
        .split('.')
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().unwrap_or(0))
        .collect();
    while n.last() == Some(&0) {
        n.pop();
    }
    (n, !suffix)
}

/// `a` is a later version than `b`.
pub fn newer(a: &str, b: &str) -> bool {
    version_key(a) > version_key(b)
}

/// The highest release with a `.raucb` bundle in GitHub's list of
/// releases (drafts never; pre-releases if `pre`).
pub fn pick(list: &Value, pre: bool) -> Option<Release> {
    let mut best: Option<Release> = None;
    for r in list.as_array()? {
        let s = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let flag = |k: &str| r.get(k).and_then(Value::as_bool).unwrap_or(false);
        if flag("draft") || (flag("prerelease") && !pre) {
            continue;
        }
        let Some(asset) = r
            .get("assets")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|a| {
                a.get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|n| n.ends_with(".raucb"))
            })
        else {
            continue;
        };
        let tag = s("tag_name");
        let version = tag.trim_start_matches(['v', 'V']).to_string();
        if version.is_empty() {
            continue;
        }
        let mut notes = s("body");
        if notes.len() > NOTES_MAX {
            let mut k = NOTES_MAX;
            while !notes.is_char_boundary(k) {
                k -= 1;
            }
            notes.truncate(k);
            notes.push_str(" …");
        }
        let rel = Release {
            version,
            name: s("name"),
            published: s("published_at"),
            url: asset
                .get("browser_download_url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            size: asset.get("size").and_then(Value::as_u64).unwrap_or(0),
            page: s("html_url"),
            notes,
            prerelease: flag("prerelease"),
        };
        if rel.url.starts_with("https://")
            && best
                .as_ref()
                .is_none_or(|b| newer(&rel.version, &b.version))
        {
            best = Some(rel);
        }
    }
    best
}

/// A line of `rauc install`'s progress: "  40% Copying image to rootfs.1".
fn progress(line: &str) -> Option<(u32, String)> {
    let (n, rest) = line.trim().split_once('%')?;
    let n: u32 = n.trim().parse().ok()?;
    Some((n.min(100), rest.trim().to_string()))
}

/// Without terminal colours.
fn plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            if it.peek() == Some(&'[') {
                it.next();
                for d in it.by_ref() {
                    if d.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// The slots of `rauc status --output-format=json`, for the page.
pub fn slots(status: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    for entry in status
        .get("slots")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(obj) = entry.as_object() else {
            continue;
        };
        for (name, s) in obj {
            let st = s.get("slot_status");
            let pick = |a: &str, b: &str| {
                st.and_then(|v| v.get(a))
                    .and_then(|v| v.get(b))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            };
            out.push(json!({
                "name": name,
                "bootname": s.get("bootname"),
                "state": s.get("state"),
                "boot_status": s.get("boot_status"),
                "version": pick("bundle", "version"),
                "installed": pick("installed", "timestamp"),
            }));
        }
    }
    // RAUC lists them in no set order
    out.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    out
}

impl Updater {
    pub fn new(sys: Sys) -> Updater {
        Updater {
            sys,
            st: Mutex::new(St::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, St> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Look for a release; the outcome is kept for the page.
    pub async fn check(&self, c: &web888_config::Update) -> Result<(), String> {
        let r = async {
            let body = crate::dxspots::fetch(
                &c.releases,
                &c.ca_file,
                "application/vnd.github+json",
                MAX_LIST,
            )
            .await?;
            let list: Value =
                serde_json::from_slice(&body).map_err(|e| format!("releases: {e}"))?;
            if !list.is_array() {
                return Err("releases: not a list".to_string());
            }
            Ok(pick(&list, c.prereleases))
        }
        .await;
        let mut st = self.lock();
        st.checked = Some(unix_now());
        match r {
            Ok(rel) => {
                st.latest = rel;
                st.check_error = None;
                Ok(())
            }
            Err(e) => {
                st.check_error = Some(e.clone());
                Err(e)
            }
        }
    }

    /// The page's view; `rauc status` is asked each time.
    pub async fn view(&self, c: &web888_config::Update) -> Value {
        let missing = self.sys.missing();
        let current = self.sys.os_version();
        let status = if missing.is_none() {
            let rauc = self.sys.rauc.clone();
            tokio::task::spawn_blocking(move || rauc_status(&rauc))
                .await
                .unwrap_or_else(|e| Err(e.to_string()))
        } else {
            Err(String::new())
        };
        let st = self.lock();
        let newer_out = match (&st.latest, &current) {
            (Some(r), Some(cur)) => newer(&r.version, cur),
            _ => false,
        };
        let i = &st.install;
        json!({
            "available": missing.is_none(),
            "why": missing,
            "enabled": c.enabled,
            "check_hours": c.check_hours,
            "current": current,
            "booted": status.as_ref().ok().and_then(|s| s.get("booted").cloned()),
            "slots": status.as_ref().map(slots).unwrap_or_default(),
            "status_error": status.as_ref().err().filter(|e| !e.is_empty()),
            "latest": st.latest.as_ref().map(Release::json),
            "newer": newer_out,
            "checked": st.checked,
            "check_error": st.check_error,
            "install": {
                "phase": match i.phase {
                    Phase::Idle => "idle",
                    Phase::Running => "running",
                    Phase::Done => "done",
                    Phase::Failed => "failed",
                },
                "version": i.version,
                "percent": i.percent,
                "step": i.step,
                "started": i.started,
                "ended": i.ended,
                "error": i.error,
                "log": i.log,
            },
        })
    }

    /// Start installing `version` (the release found last): `rauc install`
    /// on a thread of its own, its progress into the state.
    pub fn install(&'static self, version: &str) -> Result<(), String> {
        if let Some(why) = self.sys.missing() {
            return Err(why.into());
        }
        let url = {
            let mut st = self.lock();
            if st.install.phase == Phase::Running {
                return Err("an install is already running".into());
            }
            let Some(rel) = st.latest.clone().filter(|r| r.version == version) else {
                return Err(format!(
                    "{version} is not the release found: look for releases again"
                ));
            };
            st.install = Install {
                phase: Phase::Running,
                version: rel.version.clone(),
                step: "Starting".into(),
                started: unix_now(),
                ..Default::default()
            };
            rel.url
        };
        let rauc = self.sys.rauc.clone();
        std::thread::Builder::new()
            .name("rauc-install".into())
            .spawn(move || self.run_install(&rauc, &url))
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn run_install(&'static self, rauc: &Path, url: &str) {
        eprintln!("update: rauc install {url}");
        let child = Command::new(rauc)
            .arg("install")
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => return self.finish(false, format!("{}: {e}", rauc.display())),
        };
        // stderr on a thread of its own, both into the log
        let err = child.stderr.take().map(|e| {
            std::thread::spawn(move || {
                for line in BufReader::new(e).lines().map_while(Result::ok) {
                    self.line(&line);
                }
            })
        });
        if let Some(out) = child.stdout.take() {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                self.line(&line);
            }
        }
        if let Some(t) = err {
            let _ = t.join();
        }
        match child.wait() {
            Ok(s) if s.success() => self.finish(true, String::new()),
            Ok(s) => {
                let last = self
                    .lock()
                    .install
                    .log
                    .iter()
                    .rev()
                    .find(|l| progress(l).is_none() && !l.trim().is_empty())
                    .cloned()
                    .unwrap_or_else(|| format!("rauc install: {s}"));
                self.finish(false, last)
            }
            Err(e) => self.finish(false, format!("rauc install: {e}")),
        }
    }

    fn line(&self, raw: &str) {
        let line = plain(raw);
        let mut st = self.lock();
        if let Some((p, step)) = progress(&line) {
            st.install.percent = p;
            st.install.step = step;
        }
        let log = &mut st.install.log;
        log.push(line);
        if log.len() > LOG_LINES {
            log.remove(0);
        }
    }

    fn finish(&self, ok: bool, error: String) {
        let mut st = self.lock();
        let i = &mut st.install;
        i.ended = unix_now();
        if ok {
            i.phase = Phase::Done;
            i.percent = 100;
            i.step = "Installed: reboot to start it".into();
            eprintln!("update: {} installed", i.version);
        } else {
            i.phase = Phase::Failed;
            eprintln!("update: {} failed: {error}", i.version);
            i.error = error;
        }
    }

    fn running(&self) -> bool {
        self.lock().install.phase == Phase::Running
    }
}

fn rauc_status(rauc: &Path) -> Result<Value, String> {
    let out = Command::new(rauc)
        .args(["status", "--output-format=json"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("rauc status: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "rauc status: {}",
            plain(String::from_utf8_lossy(&out.stderr).trim())
        ));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("rauc status: {e}"))
}

/// Look for releases every `check_hours` while enabled, on a system with
/// RAUC. Runs until the configuration's sender is gone.
pub async fn run(mut live: watch::Receiver<Arc<Config>>) {
    let u = global();
    if u.sys.missing().is_some() {
        return;
    }
    // a little after the start: the network may come up late
    let mut wait = Duration::from_secs(120);
    loop {
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            r = live.changed() => {
                if r.is_err() {
                    return;
                }
                continue;
            }
        }
        let c = live.borrow().update.clone();
        if !c.enabled || c.check_hours == 0 {
            wait = Duration::from_secs(3600);
            continue;
        }
        let every = c.check_hours as f64 * 3600.0;
        let since = u.lock().checked.map_or(f64::INFINITY, |t| unix_now() - t);
        if since >= every {
            if let Err(e) = u.check(&c).await {
                eprintln!("update: looking for releases: {e}");
            }
            wait = Duration::from_secs_f64(every.clamp(600.0, 6.0 * 3600.0));
        } else {
            wait = Duration::from_secs_f64((every - since).clamp(60.0, 3600.0));
        }
    }
}

// ------------------------------------------------------------------ API

fn error(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(json!({ "error": msg.into() }))).into_response()
}

async fn status(_: AdminOnly, State(sh): State<Arc<Shared>>) -> Response {
    let c = sh.live.borrow().update.clone();
    Json(global().view(&c).await).into_response()
}

async fn check_now(_: AdminOnly, State(sh): State<Arc<Shared>>) -> Response {
    let c = sh.live.borrow().update.clone();
    let u = global();
    if !c.enabled {
        return error(StatusCode::CONFLICT, "updates are off (update.enabled)");
    }
    if let Some(why) = u.sys.missing() {
        return error(StatusCode::NOT_IMPLEMENTED, why);
    }
    // an error is in the view too
    let _ = u.check(&c).await;
    Json(u.view(&c).await).into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallReq {
    version: String,
}

async fn install(
    _: AdminOnly,
    State(sh): State<Arc<Shared>>,
    Json(req): Json<InstallReq>,
) -> Response {
    let c = sh.live.borrow().update.clone();
    if !c.enabled {
        return error(StatusCode::CONFLICT, "updates are off (update.enabled)");
    }
    match global().install(&req.version) {
        Ok(()) => Json(global().view(&c).await).into_response(),
        Err(e) => error(StatusCode::CONFLICT, e),
    }
}

async fn reboot(_: AdminOnly) -> Response {
    let u = global();
    if let Some(why) = u.sys.missing() {
        return error(StatusCode::NOT_IMPLEMENTED, why);
    }
    if u.running() {
        return error(
            StatusCode::CONFLICT,
            "an install is running: reboot when it is done",
        );
    }
    let cmd = u.sys.reboot.clone();
    eprintln!("update: reboot from the admin page");
    // answer first, then go
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let _ = tokio::task::spawn_blocking(move || Command::new(&cmd[0]).args(&cmd[1..]).status())
            .await;
    });
    Json(json!({})).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_in_order() {
        assert!(newer("0.9.0", "0.8.0"));
        assert!(newer("v0.10.0", "0.9.9"));
        assert!(!newer("0.8.0", "0.8"));
        assert!(!newer("0.8", "0.8.0"));
        assert!(newer("0.9.0", "0.9.0-rc1"));
        assert!(!newer("0.9.0-rc1", "0.9.0"));
        assert!(!newer("0.8.0", "0.8.0"));
        assert!(newer("1.0", "0.99.99"));
    }

    fn release(tag: &str, pre: bool, draft: bool, asset: &str) -> Value {
        json!({
            "tag_name": tag,
            "name": format!("web888-ng SD image {tag}"),
            "draft": draft,
            "prerelease": pre,
            "published_at": "2026-11-12T15:53:07Z",
            "html_url": format!("https://github.com/o/r/releases/tag/{tag}"),
            "body": "notes",
            "assets": [
                {"name": format!("web888-ng-{tag}.wic.xz"), "size": 1, "browser_download_url": "https://x/img"},
                {"name": asset, "size": 89965915, "browser_download_url": format!("https://x/{asset}")},
            ],
        })
    }

    #[test]
    fn the_highest_release_with_a_bundle() {
        let list = json!([
            release("v0.8.0", true, false, "web888-ng-v0.8.0.raucb"),
            release("v0.10.0", true, true, "web888-ng-v0.10.0.raucb"), // a draft
            release("v0.9.0", true, false, "web888-ng-v0.9.0.txt"),    // no bundle
            release("v0.7.0", false, false, "web888-ng-v0.7.0.raucb"),
        ]);
        let r = pick(&list, true).unwrap();
        assert_eq!(r.version, "0.8.0");
        assert_eq!(r.url, "https://x/web888-ng-v0.8.0.raucb");
        assert_eq!(r.size, 89965915);
        assert!(r.prerelease);
        // without pre-releases
        assert_eq!(pick(&list, false).unwrap().version, "0.7.0");
        assert_eq!(pick(&json!([]), true), None);
        assert_eq!(pick(&json!({"message": "Not Found"}), true), None);
    }

    #[test]
    fn rauc_output() {
        assert_eq!(
            progress("  40% Copying image to rootfs.1"),
            Some((40, "Copying image to rootfs.1".into()))
        );
        assert_eq!(
            progress("100% Installing done."),
            Some((100, "Installing done.".into()))
        );
        assert_eq!(progress("installing"), None);
        assert_eq!(plain("\x1b[1mbooted\x1b[0m (A)"), "booted (A)");
        let status = json!({
            "compatible": "web888",
            "booted": "A",
            "slots": [
                {"rootfs.1": {"bootname": "B", "state": "inactive", "boot_status": "bad"}},
                {"rootfs.0": {"bootname": "A", "state": "booted", "boot_status": "good",
                    "slot_status": {"bundle": {"version": "0.8.0"}, "installed": {"timestamp": "2026-11-12T20:00:00Z"}}}},
            ],
        });
        let s = slots(&status);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0]["name"], "rootfs.0");
        assert_eq!(s[0]["version"], "0.8.0");
        assert_eq!(s[1]["version"], Value::Null);
    }

    /// A fake `rauc` (a shell script) and os-release in a directory.
    fn fake(dir: &Path, script: &str) -> Sys {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir).unwrap();
        let rauc = dir.join("rauc");
        std::fs::write(&rauc, format!("#!/bin/sh\n{script}")).unwrap();
        std::fs::set_permissions(&rauc, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(dir.join("system.conf"), "[system]\n").unwrap();
        std::fs::write(dir.join("os-release"), "ID=web888\nVERSION_ID=0.8.0\n").unwrap();
        Sys {
            rauc,
            system_conf: vec![dir.join("nothing.conf"), dir.join("system.conf")],
            os_release: dir.join("os-release"),
            reboot: vec!["true".into()],
        }
    }

    fn leak(u: Updater) -> &'static Updater {
        Box::leak(Box::new(u))
    }

    async fn wait_done(u: &Updater) -> Install {
        for _ in 0..200 {
            let i = u.lock().install.clone();
            if i.phase != Phase::Running {
                return i;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the install didn't end");
    }

    #[tokio::test]
    async fn installs_the_release_found() {
        let dir = std::env::temp_dir().join(format!("web888-update-{}", std::process::id()));
        let sys = fake(
            &dir,
            r#"case "$1" in
status) echo '{"booted":"A","slots":[{"rootfs.0":{"bootname":"A","state":"booted"}}]}' ;;
install) echo installing; echo '  0% Installing'; echo " 50% Copying image to rootfs.1 ($2)"; echo '100% Installing done.'; echo "Installing \`$2' succeeded" ;;
esac
"#,
        );
        let u = leak(Updater::new(sys.clone()));
        let c = web888_config::Update::default();
        // nothing found yet: refused
        assert!(u.install("0.9.0").is_err());
        u.lock().latest = pick(&json!([release("v0.9.0", true, false, "b.raucb")]), true);
        let v = u.view(&c).await;
        assert_eq!(v["available"], true);
        assert_eq!(v["current"], "0.8.0");
        assert_eq!(v["newer"], true);
        assert_eq!(v["booted"], "A");
        assert_eq!(v["slots"][0]["bootname"], "A");
        assert!(u.install("0.8.5").is_err());
        u.install("0.9.0").unwrap();
        let i = wait_done(u).await;
        assert_eq!(i.phase, Phase::Done, "{:?}", i);
        assert_eq!(i.percent, 100);
        assert!(
            i.log
                .iter()
                .any(|l| l.contains("Copying image to rootfs.1 (https://x/b.raucb)"))
        );

        // a failing one: its last words are the error
        let sys = fake(
            &dir.join("bad"),
            "echo '  0% Installing'; echo 'signature verification failed' >&2; exit 1\n",
        );
        let u = leak(Updater::new(sys));
        u.lock().latest = pick(&json!([release("v0.9.0", true, false, "b.raucb")]), true);
        u.install("0.9.0").unwrap();
        let i = wait_done(u).await;
        assert_eq!(i.phase, Phase::Failed);
        assert_eq!(i.error, "signature verification failed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn without_rauc() {
        let u = leak(Updater::new(Sys {
            rauc: "/no/such/rauc".into(),
            ..Sys::default()
        }));
        let v = u.view(&web888_config::Update::default()).await;
        assert_eq!(v["available"], false);
        assert!(v["why"].as_str().unwrap().contains("no RAUC"));
        assert!(u.install("0.9.0").is_err());
    }

    #[tokio::test]
    async fn checks_a_releases_list() {
        // a one-shot HTTP server with GitHub's list
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let body = json!([release("v0.9.0", true, false, "web888-ng-v0.9.0.raucb")]).to_string();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut s, _) = l.accept().await.unwrap();
            let mut b = [0u8; 4096];
            let n = s.read(&mut b).await.unwrap();
            let req = String::from_utf8_lossy(&b[..n]).to_string();
            assert!(
                req.starts_with("GET /repos/o/r/releases?per_page=30 HTTP/1.1"),
                "{req}"
            );
            let r = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            s.write_all(r.as_bytes()).await.unwrap();
        });
        let u = leak(Updater::new(Sys::default()));
        let c = web888_config::Update {
            releases: format!("http://127.0.0.1:{port}/repos/o/r/releases?per_page=30"),
            ..Default::default()
        };
        u.check(&c).await.unwrap();
        let st = u.lock();
        assert_eq!(st.latest.as_ref().unwrap().version, "0.9.0");
        assert!(st.checked.is_some());
        assert_eq!(st.check_error, None);
    }
}
