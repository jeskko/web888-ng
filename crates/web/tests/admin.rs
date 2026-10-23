// SPDX-License-Identifier: GPL-3.0-or-later
//! Logins, the admin API and metrics, over HTTP on a loopback port.

use futures_util::StreamExt;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use web888_config::Config;
use web888_hw::{HwLoop, HwStatus};
use web888_radio::{Radio, r2_for_rate, rx_rate};
use web888_sim::*;
use web888_web::admin::{Admin, Hooks};
use web888_web::auth;
use web888_web::proto::{RadioInfo, ServerMsg};
use web888_web::session::Shared;

const ADMIN_PW: &str = "admin-secret-1";

/// A server whose config file is `path` (written first), reloaded when
/// the admin saves it.
async fn start(config: Config, path: PathBuf) -> (std::net::SocketAddr, Arc<AtomicBool>) {
    config.save(&path).unwrap();
    let mut radio = SimRadio::new(SimConfig::hf(Scene::new().adc_noise(), Pacing::RealTime));
    let caps = radio.caps();
    let r2 = r2_for_rate(caps.adc_hz, 48_000.0);
    radio.set_rx_rate(r2).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let status = Arc::new(HwStatus::default());
    let hw = HwLoop {
        radio: Box::new(radio),
        ctl: rx,
        stop: stop.clone(),
        status: status.clone(),
        verbose: false,
    };
    std::thread::spawn(move || hw.run());
    let channels = web888_core::leases::Channels::new(
        caps.rx_channels,
        web888_core::leases::Policy::default(),
        tx.clone(),
    );
    let pool = Arc::new(web888_core::pool::DspPool::new(2, &tx, false).unwrap());
    let (live_tx, live_rx) = tokio::sync::watch::channel(Arc::new(config));
    let p = path.clone();
    let shared = Arc::new(Shared {
        waterfalls: web888_web::wfsvc::Waterfalls::new(
            caps.wf_channels,
            caps.adc_hz,
            caps.wf_block,
            tx.clone(),
            pool.clone(),
        ),
        status,
        radio: RadioInfo {
            backend: "sim".into(),
            variant: "hf".into(),
            adc_hz: caps.adc_hz,
            rx_rate: rx_rate(caps.adc_hz, r2),
            rf_lo: *caps.rf_range.start(),
            rf_hi: *caps.rf_range.end(),
            wf_decim_min: caps.wf_decim_min,
            wf_decim_max: caps.wf_decim_max,
        },
        channels,
        pool,
        rx_r2: r2,
        live: live_rx,
        ips: Default::default(),
        sessions: Default::default(),
        next_session: Default::default(),
        users: Default::default(),
        admin: Admin::new(Hooks {
            config_path: Some(path),
            reload: Some(Box::new(move || {
                let c = Config::load(&p).map_err(|e| e.to_string())?;
                live_tx.send_replace(Arc::new(c));
                Ok(())
            })),
            restart: None,
            overlay: None,
        }),
        skim: None,
        limit_unit: Duration::from_millis(100),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = web888_web::router(shared, "/nonexistent".into());
    tokio::spawn(web888_web::serve(listener, app, None));
    (addr, stop)
}

struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{e}: {}", self.body))
    }
    /// The login token this response sets.
    fn token(&self) -> String {
        let c = self.header("set-cookie").expect("a cookie");
        let v = c.split(';').next().unwrap();
        v.strip_prefix(&format!("{}=", auth::COOKIE))
            .unwrap()
            .to_string()
    }
}

/// One HTTP/1.1 request (`Connection: close`).
async fn http(
    addr: std::net::SocketAddr,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> Resp {
    http_h(addr, method, path, token, body, &[]).await
}

/// `http` with extra request headers.
async fn http_h(
    addr: std::net::SocketAddr,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
    extra: &[(&str, &str)],
) -> Resp {
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n");
    for (k, v) in extra {
        req += &format!("{k}: {v}\r\n");
    }
    if let Some(t) = token {
        req += &format!("Cookie: {}={t}\r\n", auth::COOKIE);
    }
    if !body.is_empty() {
        req += "Content-Type: application/json\r\n";
    }
    req += &format!("Content-Length: {}\r\n\r\n{body}", body.len());
    s.write_all(req.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).await.unwrap();
    let raw = String::from_utf8(raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    let mut lines = head.lines();
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.to_string(), v.trim().to_string()))
        .collect::<Vec<_>>();
    let chunked = headers
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("transfer-encoding") && v == "chunked");
    let body = if chunked {
        // size line, data, ... 0
        let mut out = String::new();
        let mut rest = body;
        while let Some((size, tail)) = rest.split_once("\r\n") {
            let n = usize::from_str_radix(size.trim(), 16).unwrap();
            if n == 0 {
                break;
            }
            out += &tail[..n];
            rest = &tail[n + 2..];
        }
        out
    } else {
        body.to_string()
    };
    Resp {
        status,
        headers,
        body,
    }
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn ws(addr: std::net::SocketAddr, token: Option<&str>) -> Result<Ws, String> {
    ws_h(addr, token, &[]).await
}

/// `ws` with extra request headers.
async fn ws_h(
    addr: std::net::SocketAddr,
    token: Option<&str>,
    extra: &[(&'static str, &str)],
) -> Result<Ws, String> {
    let mut req = format!("ws://{addr}/ws").into_client_request().unwrap();
    for (k, v) in extra {
        req.headers_mut().insert(*k, v.parse().unwrap());
    }
    if let Some(t) = token {
        req.headers_mut()
            .insert("cookie", format!("{}={t}", auth::COOKIE).parse().unwrap());
    }
    tokio_tungstenite::connect_async(req)
        .await
        .map(|(w, _)| w)
        .map_err(|e| e.to_string())
}

/// The next JSON message.
async fn next_json(w: &mut Ws) -> Option<ServerMsg> {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(5), w.next())
            .await
            .ok()??
            .ok()?;
        if let Message::Text(t) = m {
            return Some(serde_json::from_str(&t).unwrap());
        }
        if let Message::Close(_) = m {
            return None;
        }
    }
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("web888-admin-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d.join("web888d.toml")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_api() {
    let path = temp("api");
    let mut c = Config::default();
    c.auth.admin_hash = Some(auth::hash(ADMIN_PW));
    let (addr, stop) = start(c, path.clone()).await;

    let r = http(addr, "GET", "/api/auth", None, None).await.json();
    assert_eq!(
        r,
        json!({"role": null, "admin_password": true, "listener_password": false})
    );
    for (m, p) in [
        ("GET", "/api/admin/sessions"),
        ("GET", "/api/admin/config"),
        ("GET", "/api/admin/stats"),
        ("GET", "/api/admin/skimmers"),
        ("POST", "/api/admin/sessions/1/kick"),
    ] {
        assert_eq!(http(addr, m, p, None, None).await.status, 401, "{p}");
        assert_eq!(
            http(addr, m, p, Some("forged"), None).await.status,
            401,
            "{p}"
        );
    }

    // log in
    let r = http(
        addr,
        "POST",
        "/api/login",
        None,
        Some(json!({"password": ADMIN_PW, "admin": true})),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let c = r.header("set-cookie").unwrap();
    assert!(c.contains("HttpOnly") && c.contains("SameSite=Strict") && !c.contains("Secure"));
    let admin = r.token();
    let me = |m: &'static str, p: &'static str, b: Option<Value>| {
        let t = admin.clone();
        async move { http(addr, m, p, Some(&t), b).await }
    };
    assert_eq!(me("GET", "/api/auth", None).await.json()["role"], "admin");

    // sessions: an anonymous listener and the admin's own
    // no trusted proxies: a forwarded address is ignored
    let mut anon = ws_h(addr, None, &[("x-forwarded-for", "198.51.100.99")])
        .await
        .unwrap();
    let Some(ServerMsg::Hello { class, .. }) = next_json(&mut anon).await else {
        panic!()
    };
    assert_eq!(class, "web");
    let mut own = ws(addr, Some(&admin)).await.unwrap();
    let Some(ServerMsg::Hello {
        class,
        session: own_id,
        ..
    }) = next_json(&mut own).await
    else {
        panic!()
    };
    assert_eq!(class, "admin");
    let s = me("GET", "/api/admin/sessions", None).await.json();
    let list = s["sessions"].as_array().unwrap();
    assert_eq!(list.len(), 2, "{s}");
    let anon_row = list.iter().find(|r| r["class"] == "web").unwrap();
    assert_eq!(anon_row["ip"], "127.0.0.1");
    assert!(
        anon_row["channel"].is_u64() && anon_row["hz"].is_f64(),
        "{s}"
    );
    let l = me("GET", "/api/admin/leases", None).await.json();
    assert_eq!(l["leases"].as_array().unwrap().len(), 2, "{l}");
    assert!(l["channels"].as_u64().unwrap() >= 2);

    // kick the anonymous one
    let id = anon_row["id"].as_u64().unwrap();
    assert_eq!(
        me("POST", "/api/admin/sessions/999/kick", None)
            .await
            .status,
        404
    );
    let path_kick = format!("/api/admin/sessions/{id}/kick");
    let r = http(addr, "POST", &path_kick, Some(&admin), None).await;
    assert_eq!(r.status, 200);
    let mut kicked = false;
    while let Some(m) = next_json(&mut anon).await {
        if let ServerMsg::Error { msg } = m {
            assert_eq!(msg, "disconnected by the admin");
            kicked = true;
        }
    }
    assert!(kicked);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let s = me("GET", "/api/admin/sessions", None).await.json();
    assert_eq!(s["sessions"].as_array().unwrap().len(), 1, "{s}");
    assert_eq!(s["sessions"][0]["id"], own_id);

    // the schema: sections in file order, no passwords
    assert_eq!(
        http(addr, "GET", "/api/admin/schema", None, None)
            .await
            .status,
        401
    );
    let sc = me("GET", "/api/admin/schema", None).await.json();
    let keys: Vec<&String> = sc["properties"].as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        [
            "radio", "hpsdr", "web", "limits", "station", "skimmer", "report", "syslog"
        ]
    );
    assert!(sc["$defs"].get("Auth").is_none());

    // config: no hashes out, none in; validation; saving applies it
    let g = me("GET", "/api/admin/config", None).await.json();
    assert!(g["file"].get("auth").is_none() && g["running"].get("auth").is_none());
    assert_eq!(g["auth"]["admin_password"], true);
    let mut file = g["file"].clone();
    file["limits"]["per_ip"] = json!(9);
    let v = me("POST", "/api/admin/config/validate", Some(file.clone()))
        .await
        .json();
    assert_eq!(v, json!({"live": true, "restart": []}));
    let mut bad = file.clone();
    bad["limits"]["per_ipp"] = json!(1);
    assert_eq!(
        me("POST", "/api/admin/config/validate", Some(bad))
            .await
            .status,
        400
    );
    let mut sneaky = file.clone();
    sneaky["auth"] = json!({"admin_hash": "x"});
    assert_eq!(
        me("PUT", "/api/admin/config", Some(sneaky)).await.status,
        400
    );
    let mut restart = file.clone();
    restart["web"]["listen"] = json!("0.0.0.0:80");
    let v = me("POST", "/api/admin/config/validate", Some(restart))
        .await
        .json();
    assert_eq!(v["restart"], json!(["web"]));

    // metrics: off, then switched on by saving the config
    assert_eq!(http(addr, "GET", "/metrics", None, None).await.status, 404);
    file["web"]["metrics"] = json!(true);
    let r = me("PUT", "/api/admin/config", Some(file)).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let saved = Config::load(&path).unwrap();
    assert_eq!(saved.limits.per_ip, 9);
    assert!(saved.auth.admin_hash.is_some(), "the hash stays");
    assert_eq!(me("GET", "/api/auth", None).await.json()["role"], "admin");
    let m = http(addr, "GET", "/metrics", None, None).await;
    assert_eq!(m.status, 200);
    assert!(m.body.contains("web888d_sessions 1\n"), "{}", m.body);
    assert!(m.body.contains("web888d_channels_in_use 1\n"));
    assert!(m.body.contains("web888d_pool_jobs{worker=\"1\"}"));
    assert!(m.body.contains("web888d_thread_cpu_seconds_total{thread="));

    // stats: CPU per thread name (% from the second call on)
    let s = me("GET", "/api/admin/stats", None).await.json();
    assert_eq!(s["pool"].as_array().unwrap().len(), 2);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let s = me("GET", "/api/admin/stats", None).await.json();
    let threads = s["threads"].as_array().unwrap();
    assert!(threads.iter().any(|t| t["cpu_pct"].is_f64()), "{s}");
    assert_eq!(s["sessions"], 1);

    // no skimmer service in this server
    let k = me("GET", "/api/admin/skimmers", None).await.json();
    assert_eq!(k, json!({"available": false}));

    // restart isn't wired up here
    assert_eq!(me("POST", "/api/admin/restart", None).await.status, 501);

    // a new admin password: the old logins end, the caller's continues
    let other = http(
        addr,
        "POST",
        "/api/login",
        None,
        Some(json!({"password": ADMIN_PW, "admin": true})),
    )
    .await
    .token();
    let short = json!({"role": "admin", "password": "short"});
    assert_eq!(
        me("POST", "/api/admin/password", Some(short)).await.status,
        400
    );
    let r = me(
        "POST",
        "/api/admin/password",
        Some(json!({"role": "admin", "password": "new-admin-pw"})),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let renewed = r.token();
    for (t, want) in [(&admin, 401), (&other, 401), (&renewed, 200)] {
        let r = http(addr, "GET", "/api/admin/sessions", Some(t), None).await;
        assert_eq!(r.status, want);
    }

    // logout ends a login
    let r = http(addr, "POST", "/api/logout", Some(&renewed), None).await;
    assert!(r.header("set-cookie").unwrap().contains("Max-Age=0"));
    let r = http(addr, "GET", "/api/admin/sessions", Some(&renewed), None).await;
    assert_eq!(r.status, 401);

    // wrong passwords: limited per address
    let wrong = || {
        http(
            addr,
            "POST",
            "/api/login",
            None,
            Some(json!({"password": "guess", "admin": true})),
        )
    };
    for _ in 0..auth::FAIL_LIMIT {
        assert_eq!(wrong().await.status, 401);
    }
    let r = wrong().await;
    assert_eq!(r.status, 429);
    assert!(r.header("retry-after").unwrap().parse::<u64>().unwrap() <= 60);
    // even the right one waits now
    let r = http(
        addr,
        "POST",
        "/api/login",
        None,
        Some(json!({"password": "new-admin-pw", "admin": true})),
    )
    .await;
    assert_eq!(r.status, 429);

    drop(own);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn listener_password_gates_the_receiver() {
    let path = temp("listener");
    let mut c = Config::default();
    c.auth.admin_hash = Some(auth::hash(ADMIN_PW));
    c.auth.listener_hash = Some(auth::hash("radio"));
    let (addr, stop) = start(c, path.clone()).await;

    let r = http(addr, "GET", "/api/auth", None, None).await.json();
    assert_eq!(r["listener_password"], true);
    let e = ws(addr, None).await.unwrap_err();
    assert!(e.contains("401"), "{e}");

    let login = |pw: &'static str, admin: bool| {
        http(
            addr,
            "POST",
            "/api/login",
            None,
            Some(json!({"password": pw, "admin": admin})),
        )
    };
    assert_eq!(login("wrong", false).await.status, 401);
    // the listener password doesn't make an admin
    assert_eq!(login("radio", true).await.status, 401);
    let r = login("radio", false).await;
    assert_eq!(r.json()["role"], "listener");
    let listener = r.token();
    let mut w = ws(addr, Some(&listener)).await.unwrap();
    let Some(ServerMsg::Hello { class, .. }) = next_json(&mut w).await else {
        panic!()
    };
    assert_eq!(class, "web");
    let r = http(addr, "GET", "/api/admin/sessions", Some(&listener), None).await;
    assert_eq!(r.status, 401);

    // the admin password lets one listen too; then the admin opens the
    // receiver
    let admin = login(ADMIN_PW, false).await;
    assert_eq!(admin.json()["role"], "admin");
    let r = http(
        addr,
        "POST",
        "/api/admin/password",
        Some(&admin.token()),
        Some(json!({"role": "listener", "password": null})),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(Config::load(&path).unwrap().auth.listener_hash.is_none());
    let mut anon = ws(addr, None).await.unwrap();
    assert!(matches!(
        next_json(&mut anon).await,
        Some(ServerMsg::Hello { .. })
    ));

    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// `tests/proto/schema.json` (used by the UI's tests) is the server's
/// schema; `UPDATE_FIXTURES=1` rewrites it.
#[test]
fn schema_fixture_is_current() {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/proto/schema.json");
    let mut want = serde_json::to_string_pretty(&web888_web::admin::config_schema()).unwrap();
    want.push('\n');
    if std::env::var_os("UPDATE_FIXTURES").is_some() {
        std::fs::write(&p, &want).unwrap();
    }
    let have = std::fs::read_to_string(&p).unwrap_or_default();
    assert!(
        have == want,
        "{} is stale: UPDATE_FIXTURES=1 cargo test -p web888-web --test admin",
        p.display()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trusted_proxies_forward_the_client() {
    let path = temp("proxy");
    let mut c = Config::default();
    c.auth.admin_hash = Some(auth::hash(ADMIN_PW));
    c.limits.per_ip = 1;
    c.web.trusted_proxies = vec!["127.0.0.0/8".parse().unwrap()];
    let (addr, stop) = start(c, path.clone()).await;
    let a = [("x-forwarded-for", "198.51.100.7")];
    let b = [("x-forwarded-for", "203.0.113.9, 198.51.100.8")];

    // one session per client address, the forwarded one
    let mut wa = ws_h(addr, None, &a).await.unwrap();
    assert!(matches!(
        next_json(&mut wa).await,
        Some(ServerMsg::Hello { .. })
    ));
    let mut wb = ws_h(addr, None, &b).await.unwrap();
    assert!(matches!(
        next_json(&mut wb).await,
        Some(ServerMsg::Hello { .. })
    ));
    let mut wa2 = ws_h(addr, None, &a).await.unwrap();
    assert!(matches!(
        next_json(&mut wa2).await,
        Some(ServerMsg::Busy { .. })
    ));

    // the admin sees the clients, not the proxy; https from the proxy
    // makes the cookie Secure
    let r = http_h(
        addr,
        "POST",
        "/api/login",
        None,
        Some(json!({"password": ADMIN_PW, "admin": true})),
        &[
            ("x-forwarded-for", "192.0.2.50"),
            ("x-forwarded-proto", "https"),
        ],
    )
    .await;
    assert!(r.header("set-cookie").unwrap().contains("Secure"));
    let s = http(addr, "GET", "/api/admin/sessions", Some(&r.token()), None)
        .await
        .json();
    let mut ips: Vec<&str> = s["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["ip"].as_str().unwrap())
        .collect();
    ips.sort();
    assert_eq!(ips, ["198.51.100.7", "198.51.100.8"], "{s}");

    // failed logins count per forwarded client
    let wrong = |ip: &'static str| async move {
        http_h(
            addr,
            "POST",
            "/api/login",
            None,
            Some(json!({"password": "guess", "admin": true})),
            &[("x-forwarded-for", ip)],
        )
        .await
    };
    for _ in 0..auth::FAIL_LIMIT {
        assert_eq!(wrong("192.0.2.66").await.status, 401);
    }
    assert_eq!(wrong("192.0.2.66").await.status, 429);
    assert_eq!(wrong("192.0.2.67").await.status, 401);

    drop((wa, wb, wa2));
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}
