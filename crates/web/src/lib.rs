// SPDX-License-Identifier: GPL-3.0-or-later
//! web888d's web server (`doc/m2-web-ui.md`): the UI's static files, the
//! WebSocket session (`/ws`), `/api/status`, logins and the admin API
//! (`admin`), over HTTPS and plain HTTP on one port (a TLS ClientHello is
//! told apart by its first byte).

pub mod admin;
pub mod auth;
pub mod binview;
pub mod proto;
pub mod session;
pub mod sysstat;
pub mod tls;
pub mod users;
pub mod wfsvc;
pub mod workers;

use axum::Router;
use axum::extract::{ConnectInfo, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::routing::get;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use hyper_util::service::TowerToHyperService;
use session::Shared;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;
use tower_http::compression::CompressionLayer;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;

pub use tls::Tls;

/// Request extension: the connection is TLS (cookies get `Secure`).
#[derive(Clone, Copy, Debug)]
pub struct IsTls(pub bool);

const NO_UI: &str = "<!doctype html><title>web888d</title>\
<p>web888d is running, but its web UI files are not installed \
(<code>--ui-dir</code>). The WebSocket API is at <code>/ws</code>.";

pub fn router(shared: Arc<Shared>, ui_dir: PathBuf) -> Router {
    let ui = ServeDir::new(&ui_dir).fallback(get(|| async { Html(NO_UI) }));
    let proxied = axum::middleware::from_fn_with_state(shared.clone(), client_addr);
    Router::new()
        .route("/ws", get(ws))
        .route("/api/status", get(status))
        .route("/api/spots", get(spots))
        .merge(admin::routes())
        .fallback_service(ui)
        // revalidate every load (cheap with ETags): a new UI build is
        // picked up at once, never mixed with cached old files
        .layer(SetResponseHeaderLayer::if_not_present(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-cache"),
        ))
        .layer(CompressionLayer::new())
        .layer(proxied)
        .with_state(shared)
}

/// Behind a trusted reverse proxy, the client's address and scheme from
/// X-Forwarded-For / X-Forwarded-Proto replace the proxy's, so per-IP
/// limits, login rate limits and the session list see the real client.
async fn client_addr(
    State(sh): State<Arc<Shared>>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0);
    if let Some(peer) = peer {
        let trusted = sh.live.borrow().web.trusted_proxies.clone();
        if trusted.iter().any(|n| n.contains(peer.ip())) {
            let h = req.headers();
            let client = forwarded_client(h, &trusted);
            let https = h
                .get_all("x-forwarded-proto")
                .iter()
                .filter_map(|v| v.to_str().ok())
                .flat_map(|v| v.split(','))
                .last()
                .is_some_and(|p| p.trim().eq_ignore_ascii_case("https"));
            if let Some(ip) = client {
                req.extensions_mut()
                    .insert(ConnectInfo(SocketAddr::new(ip, peer.port())));
            }
            if https {
                req.extensions_mut().insert(IsTls(true));
            }
        }
    }
    next.run(req).await
}

/// The client in X-Forwarded-For: the rightmost address that isn't a
/// trusted proxy (addresses left of it could be made up by the client).
/// None if the header is missing or holds something that isn't an
/// address where the client should be.
pub fn forwarded_client(
    h: &HeaderMap,
    trusted: &[web888_config::IpNet],
) -> Option<std::net::IpAddr> {
    let hops: Vec<&str> = h
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let mut last = None;
    for hop in hops.iter().rev() {
        let ip = parse_hop(hop)?;
        if !trusted.iter().any(|n| n.contains(ip)) {
            return Some(ip);
        }
        last = Some(ip);
    }
    // every hop was a trusted proxy: the leftmost is as far as we know
    last
}

/// "192.0.2.1", "192.0.2.1:1234", "2001:db8::1" or "[2001:db8::1]:1234".
fn parse_hop(s: &str) -> Option<std::net::IpAddr> {
    if let Ok(ip) = s.parse::<std::net::IpAddr>() {
        return Some(ip.to_canonical());
    }
    s.parse::<SocketAddr>().ok().map(|a| a.ip().to_canonical())
}

/// A WebSocket session; with a listener password set, only for logins.
async fn ws(
    up: WebSocketUpgrade,
    State(sh): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let role = admin::role(&sh, &headers);
    let open = sh.live.borrow().auth.listener_hash.is_none();
    if role.is_none() && !open {
        return (StatusCode::UNAUTHORIZED, "login required").into_response();
    }
    up.on_upgrade(move |s| session::run(s, sh, peer, role))
        .into_response()
}

async fn status(State(sh): State<Arc<Shared>>) -> impl IntoResponse {
    let s = &sh.status;
    Json(serde_json::json!({
        "radio": sh.radio,
        "sessions": sh.sessions.load(Ordering::Relaxed),
        "adc_overloads": s.overload_blocks.load(Ordering::Relaxed),
        "clock_ppm": s.clock_ppb.load(Ordering::Relaxed) as f64 / 1e3,
        "hw_rate_ok": s.rx_rate_ok.load(Ordering::Relaxed),
        "rx_frames": s.rx_frames.load(Ordering::Relaxed),
    }))
}

#[derive(serde::Deserialize)]
struct SpotQuery {
    /// UTC seconds; default: the last 30 minutes.
    since: Option<f64>,
    mode: Option<String>,
    band: Option<String>,
}

/// Recent spots (`doc/m4-skimmers.md` §5), oldest first; with a listener
/// password, for logged-in users only (as `/ws`).
async fn spots(
    State(sh): State<Arc<Shared>>,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<SpotQuery>,
) -> Response {
    let open = sh.live.borrow().auth.listener_hash.is_none();
    if admin::role(&sh, &headers).is_none() && !open {
        return (StatusCode::UNAUTHORIZED, "login required").into_response();
    }
    let Some(sk) = &sh.skim else {
        return Json(serde_json::json!({ "spots": [] })).into_response();
    };
    let since = q.since.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64()
            - 1800.0
    });
    let list: Vec<proto::SpotMsg> = sk
        .bus
        .recent(since)
        .iter()
        .map(|s| proto::SpotMsg::from(&**s))
        .filter(|s| {
            q.mode
                .as_ref()
                .is_none_or(|m| s.mode.eq_ignore_ascii_case(m))
        })
        .filter(|s| {
            q.band
                .as_ref()
                .is_none_or(|b| s.band.eq_ignore_ascii_case(b))
        })
        .collect();
    Json(serde_json::json!({ "spots": list })).into_response()
}

/// Accept connections until the listener fails: TLS when the client opens
/// with a handshake record (0x16) and a certificate is configured, plain
/// HTTP otherwise.
pub async fn serve(
    listener: TcpListener,
    app: Router,
    tls: Option<TlsAcceptor>,
) -> std::io::Result<()> {
    loop {
        let (sock, peer) = listener.accept().await?;
        let app = app.clone();
        let tls = tls.clone();
        tokio::spawn(async move {
            let _ = sock.set_nodelay(true);
            let mut first = [0u8; 1];
            let is_tls = matches!(sock.peek(&mut first).await, Ok(1) if first[0] == 0x16);
            // handlers see the client's address (as with axum's own serve)
            let app = app.map_request(move |mut r: axum::http::Request<_>| {
                r.extensions_mut().insert(ConnectInfo(peer));
                r.extensions_mut().insert(IsTls(is_tls));
                r
            });
            let svc = TowerToHyperService::new(app);
            let http = Builder::new(TokioExecutor::new());
            match (is_tls, tls) {
                (true, Some(acc)) => {
                    if let Ok(s) = acc.accept(sock).await {
                        let _ = http
                            .serve_connection_with_upgrades(TokioIo::new(s), svc)
                            .await;
                    }
                }
                (true, None) => {} // TLS not configured
                (false, _) => {
                    let _ = http
                        .serve_connection_with_upgrades(TokioIo::new(sock), svc)
                        .await;
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xff(v: &[&str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for s in v {
            h.append("x-forwarded-for", s.parse().unwrap());
        }
        h
    }

    #[test]
    fn the_client_is_the_rightmost_untrusted_hop() {
        let t: Vec<web888_config::IpNet> = vec!["10.0.0.0/8".parse().unwrap()];
        let ip = |s: &str| Some(s.parse::<std::net::IpAddr>().unwrap());
        assert_eq!(
            forwarded_client(&xff(&["198.51.100.7"]), &t),
            ip("198.51.100.7")
        );
        // a client prepending a made-up address doesn't get it believed
        assert_eq!(
            forwarded_client(&xff(&["1.2.3.4, 198.51.100.7"]), &t),
            ip("198.51.100.7")
        );
        // proxies in a chain are skipped, also across header lines
        assert_eq!(
            forwarded_client(&xff(&["198.51.100.7", "10.1.1.1"]), &t),
            ip("198.51.100.7")
        );
        assert_eq!(forwarded_client(&xff(&["10.2.2.2"]), &t), ip("10.2.2.2"));
        assert_eq!(
            forwarded_client(&xff(&["[2001:db8::5]:443"]), &t),
            ip("2001:db8::5")
        );
        assert_eq!(
            forwarded_client(&xff(&["198.51.100.7:5555"]), &t),
            ip("198.51.100.7")
        );
        assert_eq!(forwarded_client(&xff(&["unknown"]), &t), None);
        assert_eq!(forwarded_client(&HeaderMap::new(), &t), None);
    }
}
