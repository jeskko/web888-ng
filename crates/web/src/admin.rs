// SPDX-License-Identifier: GPL-3.0-or-later
//! Logins, the admin API and metrics (`doc/m3-multi-user.md` §7).
//!
//! * `GET /api/auth`: the caller's role and which passwords are set;
//!   `POST /api/login {password, admin}`, `POST /api/logout`.
//! * `/api/admin/...` (admin login): sessions (list, kick), leases,
//!   config (get, validate, put), passwords, stats, restart.
//! * `GET /metrics`: Prometheus text, when `web.metrics` is on.
//!
//! The admin cookie is SameSite=Strict and the admin API takes JSON
//! bodies only, so other sites can't make a logged-in browser call it.

use crate::IsTls;
use crate::auth::{self, Role};
use crate::session::Shared;
use axum::Router;
use axum::extract::{ConnectInfo, FromRequestParts, Path, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::Notify;
use web888_config::Config;
use web888_core::leases::{Class, LeaseId, LeaseState};
use web888_core::wf::SessionId;

/// What the server around the web crate lets the admin do.
#[derive(Default)]
pub struct Hooks {
    /// The configuration file admin edits are written to (none: the
    /// config can be read but not changed).
    pub config_path: Option<PathBuf>,
    /// Apply the file now, as a reload would.
    #[allow(clippy::type_complexity)]
    pub reload: Option<Box<dyn Fn() -> Result<(), String> + Send + Sync>>,
    /// Restart the server (it exits; the service manager starts it again).
    pub restart: Option<Box<dyn Fn() + Send + Sync>>,
    /// Lay the command-line options over a config from the file (what the
    /// server would run with it).
    #[allow(clippy::type_complexity)]
    pub overlay: Option<Box<dyn Fn(&mut Config) + Send + Sync>>,
}

/// One web session, for the admin.
pub struct Entry {
    pub ip: IpAddr,
    pub class: Class,
    pub since: SystemTime,
    pub lease: LeaseId,
    pub kick: Arc<Notify>,
    /// The session listened along to (0: none).
    pub listen: Arc<AtomicU64>,
    pub audio_drops: Arc<AtomicU64>,
    pub wf_drops: Arc<AtomicU64>,
}

#[derive(Default)]
pub struct Admin {
    pub logins: auth::Logins,
    pub sessions: Mutex<BTreeMap<SessionId, Entry>>,
    pub hooks: Hooks,
    /// Per-thread CPU at the last stats call.
    cpu: Mutex<Option<(Instant, HashMap<u32, u64>)>>,
}

impl Admin {
    pub fn new(hooks: Hooks) -> Self {
        Admin {
            hooks,
            ..Default::default()
        }
    }
}

pub fn routes() -> Router<Arc<Shared>> {
    Router::new()
        .route("/api/auth", get(whoami))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/admin/sessions", get(sessions))
        .route("/api/admin/sessions/{id}/kick", post(kick))
        .route("/api/admin/leases", get(leases))
        .route("/api/admin/config", get(config_get).put(config_put))
        .route("/api/admin/config/validate", post(config_validate))
        .route("/api/admin/schema", get(schema))
        .route("/api/admin/password", post(password))
        .route("/api/admin/stats", get(stats))
        .route("/api/admin/restart", post(restart))
        .route("/api/admin/skimmers", get(skimmers))
        .route("/metrics", get(metrics))
}

fn error(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(json!({ "error": msg.into() }))).into_response()
}

/// An error answer, small enough to pass around in a `Result`.
struct Fail(StatusCode, String);

impl Fail {
    fn new(code: StatusCode, msg: impl Into<String>) -> Self {
        Fail(code, msg.into())
    }
}

impl IntoResponse for Fail {
    fn into_response(self) -> Response {
        error(self.0, self.1)
    }
}

fn secure(tls: Option<axum::Extension<IsTls>>) -> bool {
    tls.is_some_and(|t| t.0.0)
}

/// The request's role under the live configuration.
pub fn role(sh: &Shared, headers: &HeaderMap) -> Option<Role> {
    let cfg = sh.live.borrow().clone();
    sh.admin.logins.role(headers, &cfg.auth)
}

/// Extractor: the request is from a logged-in admin.
pub struct AdminOnly;

impl FromRequestParts<Arc<Shared>> for AdminOnly {
    type Rejection = Response;
    async fn from_request_parts(p: &mut Parts, sh: &Arc<Shared>) -> Result<Self, Response> {
        match role(sh, &p.headers) {
            Some(Role::Admin) => Ok(AdminOnly),
            _ => Err(error(StatusCode::UNAUTHORIZED, "admin login required")),
        }
    }
}

async fn whoami(State(sh): State<Arc<Shared>>, headers: HeaderMap) -> Json<Value> {
    let a = sh.live.borrow().auth.clone();
    Json(json!({
        "role": sh.admin.logins.role(&headers, &a),
        "listener_password": a.listener_hash.is_some(),
        "admin_password": a.admin_hash.is_some(),
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginReq {
    password: String,
    /// Log in as the admin (else as a listener).
    #[serde(default)]
    admin: bool,
}

async fn login(
    State(sh): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    tls: Option<axum::Extension<IsTls>>,
    Json(req): Json<LoginReq>,
) -> Response {
    let ip = peer.ip();
    let want = if req.admin {
        Role::Admin
    } else {
        Role::Listener
    };
    let a = sh.live.borrow().auth.clone();
    if want == Role::Admin && a.admin_hash.is_none() {
        return error(
            StatusCode::CONFLICT,
            "no admin password is set (web888d passwd on the console)",
        );
    }
    if req.password.len() > 1024 {
        return error(StatusCode::BAD_REQUEST, "password too long");
    }
    // one check at a time; the address's record counts once it's our turn
    let _turn = sh.admin.logins.checking.lock().await;
    if let Some(d) = sh.admin.logins.blocked(ip) {
        let s = d.as_secs() + 1;
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, s.to_string())],
            Json(json!({ "error": format!("too many failed logins, try again in {s} s") })),
        )
            .into_response();
    }
    let pw = req.password;
    let got = tokio::task::spawn_blocking(move || auth::check(&a, &pw, want))
        .await
        .ok()
        .flatten();
    match got {
        Some((role, hash)) if role >= want => {
            let token = sh.admin.logins.add(role, hash);
            (
                [(
                    header::SET_COOKIE,
                    auth::set_cookie(Some(&token), secure(tls)),
                )],
                Json(json!({ "role": role })),
            )
                .into_response()
        }
        _ => {
            sh.admin.logins.failed(ip);
            error(StatusCode::UNAUTHORIZED, "wrong password")
        }
    }
}

async fn logout(
    State(sh): State<Arc<Shared>>,
    headers: HeaderMap,
    tls: Option<axum::Extension<IsTls>>,
) -> Response {
    if let Some(t) = auth::cookie(&headers) {
        sh.admin.logins.remove(t);
    }
    (
        [(header::SET_COOKIE, auth::set_cookie(None, secure(tls)))],
        Json(json!({})),
    )
        .into_response()
}

fn unix(t: SystemTime) -> f64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

fn lease_json(s: Option<LeaseState>) -> (Value, Value) {
    match s {
        Some(LeaseState::Granted { ch }) => (json!(ch), Value::Null),
        Some(LeaseState::Waiting { position }) => (Value::Null, json!(position)),
        _ => (Value::Null, Value::Null),
    }
}

async fn sessions(_: AdminOnly, State(sh): State<Arc<Shared>>) -> Json<Value> {
    let leases: HashMap<_, _> = sh
        .channels
        .leases()
        .into_iter()
        .map(|l| (l.id, l.state))
        .collect();
    let list: Vec<Value> = sh
        .admin
        .sessions
        .lock()
        .unwrap()
        .iter()
        .map(|(&id, e)| {
            let p = sh.users.get(id);
            let (channel, waiting) = lease_json(leases.get(&e.lease).copied());
            let listen = e.listen.load(Ordering::Relaxed);
            json!({
                "id": id,
                "ip": e.ip,
                "class": e.class.name(),
                "since": unix(e.since),
                "hz": p.map(|p| p.hz),
                "mode": p.map(|p| p.mode),
                "share": p.map(|p| p.share),
                "channel": channel,
                "waiting": waiting,
                "listen": (listen != 0).then_some(listen),
                "audio_drops": e.audio_drops.load(Ordering::Relaxed),
                "wf_drops": e.wf_drops.load(Ordering::Relaxed),
            })
        })
        .collect();
    Json(json!({ "sessions": list }))
}

async fn kick(_: AdminOnly, State(sh): State<Arc<Shared>>, Path(id): Path<SessionId>) -> Response {
    match sh.admin.sessions.lock().unwrap().get(&id) {
        Some(e) => {
            e.kick.notify_one();
            Json(json!({})).into_response()
        }
        None => error(StatusCode::NOT_FOUND, format!("no session {id}")),
    }
}

async fn leases(_: AdminOnly, State(sh): State<Arc<Shared>>) -> Json<Value> {
    let list: Vec<Value> = sh
        .channels
        .leases()
        .into_iter()
        .map(|l| {
            let (channel, waiting) = lease_json(Some(l.state));
            json!({
                "id": l.id,
                "class": l.class.name(),
                "label": l.label,
                "hz": l.hz,
                "channel": channel,
                "waiting": waiting,
            })
        })
        .collect();
    Json(json!({ "channels": sh.channels.channels(), "leases": list }))
}

/// A config as JSON without its password hashes.
fn public(c: &Config) -> Value {
    let mut v = serde_json::to_value(c).expect("config serialises");
    if let Some(o) = v.as_object_mut() {
        o.remove("auth");
    }
    v
}

fn file_config(sh: &Shared) -> Result<Config, Fail> {
    match &sh.admin.hooks.config_path {
        Some(p) => {
            Config::load(p).map_err(|e| Fail::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
        }
        None => Ok(Config::default()),
    }
}

async fn config_get(_: AdminOnly, State(sh): State<Arc<Shared>>) -> Response {
    let file = match file_config(&sh) {
        Ok(c) => c,
        Err(r) => return r.into_response(),
    };
    let running = sh.live.borrow().clone();
    Json(json!({
        "path": sh.admin.hooks.config_path,
        "writable": sh.admin.hooks.config_path.is_some(),
        "file": public(&file),
        "running": public(&running),
        "auth": {
            "admin_password": running.auth.admin_hash.is_some(),
            "listener_password": running.auth.listener_hash.is_some(),
        },
    }))
    .into_response()
}

async fn schema(_: AdminOnly) -> Json<Value> {
    Json(config_schema())
}

/// The config's JSON Schema, without `auth` (as the API's configs).
pub fn config_schema() -> Value {
    let mut v = serde_json::to_value(Config::schema()).expect("schema serialises");
    if let Some(p) = v.get_mut("properties").and_then(Value::as_object_mut) {
        p.remove("auth");
    }
    if let Some(d) = v.get_mut("$defs").and_then(Value::as_object_mut) {
        d.remove("Auth");
    }
    v
}

/// A config from the API (no `auth`: passwords have their own call).
fn parse(v: Value) -> Result<Config, Fail> {
    if v.get("auth").is_some() {
        return Err(Fail::new(
            StatusCode::BAD_REQUEST,
            "passwords are set with /api/admin/password, not in the config",
        ));
    }
    serde_json::from_value(v).map_err(|e| Fail::new(StatusCode::BAD_REQUEST, e.to_string()))
}

/// What saving `file` would change in the running server.
fn changes_json(sh: &Shared, file: &Config) -> Value {
    let mut new = file.clone();
    if let Some(o) = &sh.admin.hooks.overlay {
        o(&mut new);
    }
    let ch = sh.live.borrow().changes(&new);
    json!({ "live": ch.live, "restart": ch.restart })
}

async fn config_validate(
    _: AdminOnly,
    State(sh): State<Arc<Shared>>,
    Json(v): Json<Value>,
) -> Response {
    match parse(v) {
        Ok(mut c) => {
            c.auth = sh.live.borrow().auth.clone();
            Json(changes_json(&sh, &c)).into_response()
        }
        Err(r) => r.into_response(),
    }
}

/// Write `c` to the config file and apply it.
fn save(sh: &Shared, c: &Config) -> Result<(), Fail> {
    let Some(path) = &sh.admin.hooks.config_path else {
        return Err(Fail::new(
            StatusCode::NOT_IMPLEMENTED,
            "this server has no config file to write",
        ));
    };
    c.save(path)
        .map_err(|e| Fail::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if let Some(reload) = &sh.admin.hooks.reload {
        reload().map_err(|e| Fail::new(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    }
    Ok(())
}

async fn config_put(_: AdminOnly, State(sh): State<Arc<Shared>>, Json(v): Json<Value>) -> Response {
    let mut c = match parse(v) {
        Ok(c) => c,
        Err(r) => return r.into_response(),
    };
    let file = match file_config(&sh) {
        Ok(f) => f,
        Err(r) => return r.into_response(),
    };
    c.auth = file.auth;
    let changes = changes_json(&sh, &c);
    match save(&sh, &c) {
        Ok(()) => Json(changes).into_response(),
        Err(r) => r.into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PasswordReq {
    role: Role,
    /// None: remove the listener password (an open receiver).
    password: Option<String>,
}

async fn password(
    _: AdminOnly,
    State(sh): State<Arc<Shared>>,
    tls: Option<axum::Extension<IsTls>>,
    Json(req): Json<PasswordReq>,
) -> Response {
    let pw = match (req.role, req.password) {
        (Role::Admin, None) => {
            return error(
                StatusCode::BAD_REQUEST,
                "the admin password can't be removed",
            );
        }
        (Role::Admin, Some(p)) if p.chars().count() < auth::ADMIN_MIN_LEN => {
            return error(
                StatusCode::BAD_REQUEST,
                format!("at least {} characters", auth::ADMIN_MIN_LEN),
            );
        }
        (_, Some(p)) if p.is_empty() || p.len() > 1024 => {
            return error(StatusCode::BAD_REQUEST, "1 to 1024 bytes");
        }
        (_, p) => p,
    };
    let hash = match pw {
        Some(p) => {
            let _turn = sh.admin.logins.checking.lock().await;
            Some(
                tokio::task::spawn_blocking(move || auth::hash(&p))
                    .await
                    .expect("hashing"),
            )
        }
        None => None,
    };
    let mut c = match file_config(&sh) {
        Ok(c) => c,
        Err(r) => return r.into_response(),
    };
    match req.role {
        Role::Admin => c.auth.admin_hash = hash.clone(),
        Role::Listener => c.auth.listener_hash = hash.clone(),
    }
    if let Err(r) = save(&sh, &c) {
        return r.into_response();
    }
    if req.role == Role::Admin {
        // the old password's logins end; this one continues on the new
        let token = sh.admin.logins.add(Role::Admin, hash.expect("set above"));
        return (
            [(
                header::SET_COOKIE,
                auth::set_cookie(Some(&token), secure(tls)),
            )],
            Json(json!({})),
        )
            .into_response();
    }
    Json(json!({})).into_response()
}

/// Per-thread CPU time (clock ticks) and names, from `/proc/self/task`.
fn threads() -> Vec<(u32, String, u64)> {
    let Ok(dir) = std::fs::read_dir("/proc/self/task") else {
        return vec![];
    };
    dir.filter_map(|e| {
        let e = e.ok()?;
        let tid: u32 = e.file_name().to_str()?.parse().ok()?;
        let stat = std::fs::read_to_string(e.path().join("stat")).ok()?;
        let (head, rest) = stat.rsplit_once(')')?;
        let name = head.split_once('(')?.1.to_string();
        // fields after the name: state is 3, utime 14, stime 15
        let f: Vec<&str> = rest.split_whitespace().collect();
        let ticks = f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?;
        Some((tid, name, ticks))
    })
    .collect()
}

fn clock_ticks() -> f64 {
    // SAFETY: sysconf has no preconditions
    let t = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if t > 0 { t as f64 } else { 100.0 }
}

/// CPU per thread name: (seconds in all, % of a core since the last call).
fn cpu(sh: &Shared) -> BTreeMap<String, (f64, Option<f64>)> {
    let now = Instant::now();
    let hz = clock_ticks();
    let t = threads();
    let mut last = sh.admin.cpu.lock().unwrap();
    let mut out: BTreeMap<String, (f64, Option<f64>)> = BTreeMap::new();
    for (tid, name, ticks) in &t {
        let pct = last.as_ref().map(|(t0, prev)| {
            let dt = (now - *t0).as_secs_f64().max(1e-3);
            let d = ticks.saturating_sub(prev.get(tid).copied().unwrap_or(0));
            100.0 * d as f64 / hz / dt
        });
        let e = out.entry(name.clone()).or_default();
        e.0 += *ticks as f64 / hz;
        e.1 = match (e.1, pct) {
            (Some(a), Some(b)) => Some(a + b),
            (a, b) => a.or(b),
        };
    }
    *last = Some((now, t.into_iter().map(|(tid, _, k)| (tid, k)).collect()));
    out
}

async fn stats(_: AdminOnly, State(sh): State<Arc<Shared>>) -> Json<Value> {
    let threads: Vec<Value> = cpu(&sh)
        .into_iter()
        .map(|(name, (s, pct))| json!({ "name": name, "cpu_s": s, "cpu_pct": pct }))
        .collect();
    let pool: Vec<Value> = sh
        .pool
        .stats()
        .iter()
        .map(|w| {
            json!({
                "jobs": w.jobs,
                "busy_s": w.busy.as_secs_f64(),
                "rx_dropped": w.rx_dropped,
                "wf_dropped": w.wf_dropped,
                "shed": w.shed,
            })
        })
        .collect();
    let views: Vec<Value> = sh
        .waterfalls
        .views()
        .iter()
        .map(|v| {
            json!({
                "hz": v.hz,
                "decim": v.decim,
                "fps": v.fps,
                "slot": v.slot,
                "shared": v.shared,
                "sessions": v.sessions,
            })
        })
        .collect();
    let s = &sh.status;
    Json(json!({
        "sessions": sh.sessions.load(Ordering::Relaxed),
        "threads": threads,
        "pool": pool,
        "wf_views": views,
        "hw": {
            "adc_overloads": s.overload_blocks.load(Ordering::Relaxed),
            "clock_ppm": s.clock_ppb.load(Ordering::Relaxed) as f64 / 1e3,
            "hw_rate_ok": s.rx_rate_ok.load(Ordering::Relaxed),
            "rx_frames": s.rx_frames.load(Ordering::Relaxed),
        },
    }))
}

/// The configured skimmer bands, what the running ones did, and the
/// uploaders.
async fn skimmers(_: AdminOnly, State(sh): State<Arc<Shared>>) -> Json<Value> {
    let Some(sk) = &sh.skim else {
        return Json(json!({ "available": false }));
    };
    let cfg = sh.live.borrow().clone();
    let running = sk.skimmers.as_ref().map(|s| s.bands()).unwrap_or_default();
    let bands: Vec<Value> = cfg
        .skimmer
        .iter()
        .map(|c| {
            let mut v = json!({
                "mode": c.mode,
                "dial": c.dial,
                "band": web888_skim::spot::band_name(c.dial),
                "hours": c.hours,
                "enabled": c.enabled,
                "running": false,
            });
            if let Some(b) = running.iter().find(|b| b.cfg == *c) {
                let st = &b.status;
                let n = |a: &AtomicU64| a.load(Ordering::Relaxed);
                let slots = n(&st.slots);
                v["running"] = json!(true);
                v["granted"] = json!(st.granted.load(Ordering::Relaxed));
                v["slots"] = json!(slots);
                v["spots"] = json!(n(&st.spots));
                v["cut_short"] = json!(n(&st.cut_short));
                v["dropped"] = json!(n(&st.dropped));
                v["cpu_s"] = json!(n(&st.cpu_us) as f64 / 1e6);
                v["cpu_per_slot_s"] = json!(n(&st.cpu_us) as f64 / 1e6 / slots.max(1) as f64);
                v["last_slot"] = json!(Some(n(&st.last_slot)).filter(|&t| t > 0));
            }
            v
        })
        .collect();
    let psk = sk.pskreporter.as_ref().map(|p| {
        let p = p.lock().unwrap().clone();
        json!({
            "enabled": p.enabled,
            "pending": p.pending,
            "datagrams": p.datagrams,
            "reports": p.reports,
            "skipped_not_live": p.skipped_not_live,
            "skipped_recent": p.skipped_recent,
            "last_sent": p.last_sent,
            "last_error": p.last_error,
        })
    });
    let wsprnet = sk.wsprnet.as_ref().map(|w| {
        let w = w.lock().unwrap().clone();
        json!({
            "enabled": w.enabled,
            "pending": w.pending,
            "sent": w.sent,
            "failed": w.failed,
            "skipped_not_live": w.skipped_not_live,
            "last_sent": w.last_sent,
            "last_reply": w.last_reply,
            "last_error": w.last_error,
        })
    });
    Json(json!({
        "available": true,
        "live": sk.live,
        "wsprnet": wsprnet,
        "callsign": cfg.station.callsign,
        "locator": cfg.station.locator,
        "bands": bands,
        "pskreporter": psk,
    }))
}

async fn restart(_: AdminOnly, State(sh): State<Arc<Shared>>) -> Response {
    let Some(_) = &sh.admin.hooks.restart else {
        return error(StatusCode::NOT_IMPLEMENTED, "restart is not available");
    };
    // answer first, then go
    let sh2 = sh.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        if let Some(r) = &sh2.admin.hooks.restart {
            r();
        }
    });
    Json(json!({})).into_response()
}

async fn metrics(State(sh): State<Arc<Shared>>) -> Response {
    if !sh.live.borrow().web.metrics {
        return StatusCode::NOT_FOUND.into_response();
    }
    use std::fmt::Write;
    let mut m = String::new();
    let mut put = |name: &str, kind: &str, help: &str, rows: &[(String, f64)]| {
        let _ = writeln!(
            m,
            "# HELP web888d_{name} {help}\n# TYPE web888d_{name} {kind}"
        );
        for (labels, v) in rows {
            let _ = writeln!(m, "web888d_{name}{labels} {v}");
        }
    };
    let one = |v: f64| vec![(String::new(), v)];
    let s = &sh.status;
    put(
        "sessions",
        "gauge",
        "Web sessions.",
        &one(sh.sessions.load(Ordering::Relaxed) as f64),
    );
    let leases = sh.channels.leases();
    let granted = leases
        .iter()
        .filter(|l| l.state.channel().is_some())
        .count();
    put(
        "channels",
        "gauge",
        "RX channels.",
        &one(sh.channels.channels() as f64),
    );
    put(
        "channels_in_use",
        "gauge",
        "RX channels leased.",
        &one(granted as f64),
    );
    put(
        "lease_queue",
        "gauge",
        "Lease requests waiting for a channel.",
        &one((leases.len() - granted) as f64),
    );
    put(
        "adc_overloads_total",
        "counter",
        "Blocks with an ADC overload.",
        &one(s.overload_blocks.load(Ordering::Relaxed) as f64),
    );
    put(
        "rx_frames_total",
        "counter",
        "RX frames read from the hardware.",
        &one(s.rx_frames.load(Ordering::Relaxed) as f64),
    );
    put(
        "clock_ppm",
        "gauge",
        "ADC clock error (GPS PPS).",
        &one(s.clock_ppb.load(Ordering::Relaxed) as f64 / 1e3),
    );
    put(
        "hw_rate_ok",
        "gauge",
        "The hardware keeps up with the RX rate.",
        &one(s.rx_rate_ok.load(Ordering::Relaxed) as u8 as f64),
    );
    let pool = sh.pool.stats();
    let per = |f: &dyn Fn(&web888_core::pool::WorkerStats) -> f64| -> Vec<(String, f64)> {
        pool.iter()
            .enumerate()
            .map(|(k, w)| (format!("{{worker=\"{k}\"}}"), f(w)))
            .collect()
    };
    put(
        "pool_busy_seconds_total",
        "counter",
        "DSP worker busy time.",
        &per(&|w| w.busy.as_secs_f64()),
    );
    put(
        "pool_jobs",
        "gauge",
        "Jobs on a DSP worker.",
        &per(&|w| w.jobs as f64),
    );
    put(
        "pool_shed_level",
        "gauge",
        "DSP worker load shedding level.",
        &per(&|w| w.shed as f64),
    );
    put(
        "pool_rx_dropped_total",
        "counter",
        "RX blocks a DSP worker lost.",
        &per(&|w| w.rx_dropped as f64),
    );
    put(
        "pool_wf_dropped_total",
        "counter",
        "WF blocks a DSP worker lost.",
        &per(&|w| w.wf_dropped as f64),
    );
    if let Some(sk) = sh.skim.as_ref().and_then(|k| k.skimmers.as_ref()) {
        let bands = sk.bands();
        let per = |f: &dyn Fn(&web888_skim::BandStatus) -> f64| -> Vec<(String, f64)> {
            bands
                .iter()
                .map(|b| (format!("{{band=\"{}\"}}", b.label), f(&b.status)))
                .collect()
        };
        let n = |a: &AtomicU64| a.load(Ordering::Relaxed) as f64;
        put(
            "skim_slots_total",
            "counter",
            "Slots a skimmer band decoded.",
            &per(&|s| n(&s.slots)),
        );
        put(
            "skim_spots_total",
            "counter",
            "Spots a skimmer band decoded.",
            &per(&|s| n(&s.spots)),
        );
        put(
            "skim_slots_dropped_total",
            "counter",
            "Slots dropped: the decoders were behind.",
            &per(&|s| n(&s.dropped)),
        );
        put(
            "skim_cpu_seconds_total",
            "counter",
            "Decoder CPU time of a skimmer band.",
            &per(&|s| n(&s.cpu_us) / 1e6),
        );
    }
    if let Some(p) = sh.skim.as_ref().and_then(|k| k.pskreporter.as_ref()) {
        let p = p.lock().unwrap().clone();
        put(
            "pskreporter_reports_total",
            "counter",
            "Spots sent to PSKReporter.",
            &one(p.reports as f64),
        );
    }
    if let Some(w) = sh.skim.as_ref().and_then(|k| k.wsprnet.as_ref()) {
        let w = w.lock().unwrap().clone();
        put(
            "wsprnet_spots_total",
            "counter",
            "WSPR spots sent to WSPRnet.",
            &one(w.sent as f64),
        );
    }
    let hz = clock_ticks();
    let mut by_name: BTreeMap<String, u64> = BTreeMap::new();
    for (_, name, ticks) in threads() {
        *by_name.entry(name).or_default() += ticks;
    }
    let rows: Vec<(String, f64)> = by_name
        .into_iter()
        .map(|(n, t)| {
            (
                format!("{{thread=\"{}\"}}", n.replace(['"', '\\'], "_")),
                t as f64 / hz,
            )
        })
        .collect();
    put(
        "thread_cpu_seconds_total",
        "counter",
        "CPU time per thread name.",
        &rows,
    );
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], m).into_response()
}
