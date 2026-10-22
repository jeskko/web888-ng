// SPDX-License-Identifier: GPL-3.0-or-later
//! web888d's web server (`doc/m2-web-ui.md`): the UI's static files, the
//! WebSocket session (`/ws`) and `/api/status`, over HTTPS and plain HTTP
//! on one port (a TLS ClientHello is told apart by its first byte).

pub mod proto;
pub mod session;
pub mod tls;
pub mod wfsvc;
pub mod workers;

use axum::Router;
use axum::extract::{ConnectInfo, State, WebSocketUpgrade};
use axum::response::{Html, IntoResponse, Json};
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

const NO_UI: &str = "<!doctype html><title>web888d</title>\
<p>web888d is running, but its web UI files are not installed \
(<code>--ui-dir</code>). The WebSocket API is at <code>/ws</code>.";

pub fn router(shared: Arc<Shared>, ui_dir: PathBuf) -> Router {
    let ui = ServeDir::new(&ui_dir).fallback(get(|| async { Html(NO_UI) }));
    Router::new()
        .route("/ws", get(ws))
        .route("/api/status", get(status))
        .fallback_service(ui)
        // revalidate every load (cheap with ETags): a new UI build is
        // picked up at once, never mixed with cached old files
        .layer(SetResponseHeaderLayer::if_not_present(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-cache"),
        ))
        .layer(CompressionLayer::new())
        .with_state(shared)
}

async fn ws(
    up: WebSocketUpgrade,
    State(sh): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    up.on_upgrade(move |s| session::run(s, sh, peer))
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
