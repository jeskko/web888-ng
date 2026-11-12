// SPDX-License-Identifier: GPL-3.0-or-later
//! Live DX spots (`[dxspots]`): web888d follows a Spothole server
//! (<https://spothole.app>, an aggregator of DX clusters and outdoor
//! activity programmes; API: its `/apidocs`) and serves the spots in the
//! receiver's range as the `dx` labels of `/api/markers`.
//!
//! One connection per receiver, whatever the number of listeners, as
//! Spothole asks of its clients: at start (and after each reconnect) the
//! spots of the last `max_age_minutes` (`GET /spots`), then each new one
//! as it comes (`GET /spots/stream`, server-sent events). A failure waits
//! 30 s before the next try, doubling up to 10 min; the User-Agent names
//! web888d. Nothing is sent to Spothole but these requests.
//!
//! HTTP/1.1 is spoken here (a request, a response with a length, chunks
//! or up to the close): enough for this API, without an HTTP client
//! crate.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::watch;
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName};
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use web888_config::{Config, DxSpots};
use web888_markers::{Marker, Tune};

/// The roots used when neither `ca_file` nor the system has any.
const BUILTIN_ROOTS: &[u8] = include_bytes!("../data/isrg-roots.pem");
/// System CA bundles, the first found.
const SYSTEM_ROOTS: [&str; 3] = [
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/cert.pem",
];
const FIRST_RETRY: Duration = Duration::from_secs(30);
const LAST_RETRY: Duration = Duration::from_secs(600);
/// A connection that lasted this long was a good one: the next failure
/// waits the first delay again.
const GOOD_RUN: Duration = Duration::from_secs(300);
/// The stream is given up (and opened again) after this long silent.
const IDLE: Duration = Duration::from_secs(600);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// Most spots held, and the biggest backlog read.
const KEEP: usize = 10_000;
const MAX_BODY: usize = 16 << 20;

/// A spot as kept here.
#[derive(Clone, Debug, PartialEq)]
pub struct DxSpot {
    pub id: String,
    /// When spotted, Unix time.
    pub time: f64,
    pub hz: f64,
    pub band: String,
    pub dx: String,
    pub de: String,
    /// "CW", "SSB", "FT8"... (may be inferred by Spothole from the band
    /// plan), and its family ("CW", "PHONE", "DATA").
    pub mode: String,
    pub mode_type: String,
    pub comment: String,
    /// "Cluster", "POTA", ...
    pub source: String,
    /// Activity references ("SOTA W5N/NL-004", ...).
    pub refs: Vec<String>,
    /// Where the station is (latitude, longitude), when Spothole thinks
    /// it good enough for a map (a reference's place, a grid, or the
    /// country's).
    pub dx_pos: Option<(f64, f64)>,
    /// Where the spotter is (often its country's centre).
    pub de_pos: Option<(f64, f64)>,
}

impl DxSpot {
    /// From Spothole's JSON spot; none if it lacks a call or a frequency
    /// or is marked QRT.
    pub fn from_json(v: &serde_json::Value) -> Option<DxSpot> {
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        if v.get("qrt").and_then(|q| q.as_bool()) == Some(true) {
            return None;
        }
        let hz = v.get("freq")?.as_f64()?;
        let dx = s("dx_call");
        if dx.is_empty() || hz.is_nan() || hz <= 0.0 {
            return None;
        }
        let time = v
            .get("time")
            .and_then(|t| t.as_f64())
            .or_else(|| v.get("received_time").and_then(|t| t.as_f64()))
            .unwrap_or(0.0);
        let refs = v
            .get("activity_refs")
            .and_then(|r| r.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|r| {
                        let id = r.get("id")?.as_str()?;
                        let act = r.get("activity").and_then(|x| x.as_str()).unwrap_or("");
                        Some(format!("{act} {id}").trim().to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();
        let pos = |lat: &str, lon: &str| {
            let la = v.get(lat)?.as_f64()?;
            let lo = v.get(lon)?.as_f64()?;
            ((-90.0..=90.0).contains(&la) && (-180.0..=180.0).contains(&lo)).then_some((la, lo))
        };
        let good = v.get("dx_location_good").and_then(|g| g.as_bool()) != Some(false);
        // a locator of RR73 is FT8's sign-off read as one
        let rr73 = s("dx_grid").to_ascii_uppercase().starts_with("RR73");
        let dx_pos = pos("dx_latitude", "dx_longitude").filter(|_| good && !rr73);
        let de_pos = pos("de_latitude", "de_longitude");
        let mut id = s("id");
        if id.is_empty() {
            id = format!("{dx} {hz} {time}");
        }
        Some(DxSpot {
            id,
            time,
            hz,
            band: s("band"),
            dx,
            de: s("de_call"),
            mode: s("mode"),
            mode_type: s("mode_type"),
            comment: s("comment"),
            source: s("source"),
            refs,
            dx_pos,
            de_pos,
        })
    }

    /// How the receiver tunes to it.
    pub fn tune(&self) -> Tune {
        let sideband = if self.hz < 10e6 { Tune::Lsb } else { Tune::Usb };
        match self.mode.to_ascii_uppercase().as_str() {
            "CW" => Tune::Cw,
            "AM" => Tune::Am,
            "FM" => Tune::Nbfm,
            "LSB" => Tune::Lsb,
            "USB" => Tune::Usb,
            "SSB" | "PHONE" => sideband,
            "" => match self.mode_type.to_ascii_uppercase().as_str() {
                "CW" => Tune::Cw,
                "DATA" => Tune::Usb,
                _ => sideband,
            },
            // FT8, RTTY, ...: data, upper sideband
            _ => Tune::Usb,
        }
    }

    pub fn marker(&self) -> Marker {
        let mut note = vec![];
        if !self.comment.is_empty() {
            note.push(self.comment.clone());
        }
        if !self.refs.is_empty() {
            note.push(self.refs.join(", "));
        }
        if !self.de.is_empty() {
            note.push(format!("de {}", self.de));
        }
        if !self.source.is_empty() && self.source != "Cluster" {
            note.push(format!("via {}", self.source));
        }
        let secs = self.time as i64;
        Marker {
            hz: self.hz,
            dial: None,
            name: self.dx.clone(),
            mode: self.tune(),
            emission: self.mode.to_ascii_lowercase(),
            category: "dx".into(),
            params: None,
            note: note.join(" · "),
            schedule: format!(
                "spotted {:02}:{:02}",
                secs.rem_euclid(86_400) / 3600,
                secs.rem_euclid(3600) / 60
            ),
            src: "dx".into(),
        }
    }
}

#[derive(Default)]
struct Store {
    spots: VecDeque<DxSpot>,
    ids: HashSet<String>,
}

/// The spots and how the connection is doing.
#[derive(Default)]
pub struct Dx {
    store: Mutex<Store>,
    /// The stream is open.
    pub connected: AtomicBool,
    /// Following (`enabled`).
    pub enabled: AtomicBool,
    /// Spots taken in (not repeats), and failed connections.
    pub received: AtomicU64,
    pub errors: AtomicU64,
    pub last_error: Mutex<Option<String>>,
}

/// The receiver's spots (what `/api/markers` and the metrics read).
pub fn global() -> &'static Dx {
    static G: OnceLock<Dx> = OnceLock::new();
    G.get_or_init(Dx::default)
}

impl Dx {
    /// Take a spot in (once per id), keeping at most [`KEEP`].
    pub fn add(&self, s: DxSpot) -> bool {
        let mut st = self.store.lock().unwrap();
        if !st.ids.insert(s.id.clone()) {
            return false;
        }
        st.spots.push_back(s);
        while st.spots.len() > KEEP {
            if let Some(old) = st.spots.pop_front() {
                st.ids.remove(&old.id);
            }
        }
        self.received.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// Drop spots spotted before `oldest` (Unix time).
    pub fn prune(&self, oldest: f64) {
        let mut st = self.store.lock().unwrap();
        let Store { spots, ids } = &mut *st;
        spots.retain(|s| {
            let keep = s.time >= oldest;
            if !keep {
                ids.remove(&s.id);
            }
            keep
        });
    }

    pub fn clear(&self) {
        *self.store.lock().unwrap() = Store::default();
    }

    pub fn len(&self) -> usize {
        self.store.lock().unwrap().spots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The latest spot of each station on each band spotted at or after
    /// `oldest` (Unix time), newest first, at most `max`.
    pub fn latest(&self, oldest: f64, max: usize) -> Vec<DxSpot> {
        let st = self.store.lock().unwrap();
        let mut all: Vec<&DxSpot> = st.spots.iter().filter(|s| s.time >= oldest).collect();
        all.sort_by(|a, b| b.time.total_cmp(&a.time));
        let mut seen: HashSet<(&str, &str)> = HashSet::new();
        all.into_iter()
            .filter(|s| seen.insert((s.dx.as_str(), s.band.as_str())))
            .take(max)
            .cloned()
            .collect()
    }

    /// The labels in `lo..=hi` Hz: the latest spot of each station on
    /// each band, spotted at or after `oldest` (Unix time).
    pub fn markers(&self, lo: f64, hi: f64, oldest: f64) -> Vec<Marker> {
        let st = self.store.lock().unwrap();
        let mut seen: HashSet<(&str, &str)> = HashSet::new();
        let mut out: Vec<&DxSpot> = st.spots.iter().filter(|s| s.time >= oldest).collect();
        // the newest first, then one per station and band
        out.sort_by(|a, b| b.time.total_cmp(&a.time));
        let mut m: Vec<Marker> = out
            .into_iter()
            .filter(|s| seen.insert((s.dx.as_str(), s.band.as_str())))
            .filter(|s| s.hz >= lo && s.hz <= hi)
            .map(DxSpot::marker)
            .collect();
        m.sort_by(|a, b| a.hz.total_cmp(&b.hz));
        m
    }

    fn failed(&self, e: &str) {
        self.errors.fetch_add(1, Ordering::Relaxed);
        *self.last_error.lock().unwrap() = Some(e.to_string());
    }
}

fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// Follow `[dxspots]` as it is configured (live), for a receiver covering
/// `rf_lo..rf_hi` Hz. Runs until the configuration's sender is gone.
pub async fn run(mut live: watch::Receiver<Arc<Config>>, rf_lo: f64, rf_hi: f64) {
    let dx = global();
    let mut retry = FIRST_RETRY;
    let mut warned = String::new();
    loop {
        let c = live.borrow_and_update().dxspots.clone();
        dx.enabled.store(c.enabled, Ordering::Relaxed);
        dx.connected.store(false, Ordering::Relaxed);
        if !c.enabled {
            dx.clear();
            if live.changed().await.is_err() {
                return;
            }
            continue;
        }
        let started = Instant::now();
        let r = tokio::select! {
            r = follow(dx, &c, rf_lo, rf_hi) => r,
            ch = live.changed() => {
                if ch.is_err() {
                    return;
                }
                // a new configuration: start over with it
                dx.clear();
                retry = FIRST_RETRY;
                continue;
            }
        };
        dx.connected.store(false, Ordering::Relaxed);
        if started.elapsed() >= GOOD_RUN {
            retry = FIRST_RETRY;
        }
        let wait = match r {
            // the stream ended or went quiet: open it again soon
            Ok(()) => Duration::from_secs(5),
            Err(e) => {
                dx.failed(&e);
                let w = format!("dxspots: {}: {e}", c.server);
                if w != warned {
                    eprintln!("{w}; trying again in {} s", retry.as_secs());
                    warned = w;
                }
                let w = retry;
                retry = (retry * 2).min(LAST_RETRY);
                w
            }
        };
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            ch = live.changed() => {
                if ch.is_err() {
                    return;
                }
                retry = FIRST_RETRY;
            }
        }
    }
}

/// The query string of the configured filters.
fn filters(c: &DxSpots) -> String {
    let mut q = vec![];
    let list = |v: &[String]| {
        v.iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(enc)
            .collect::<Vec<_>>()
            .join(",")
    };
    let src = list(&c.sources);
    if !src.is_empty() {
        q.push(format!("source={src}"));
    }
    let de = list(&c.de_continents);
    if !de.is_empty() {
        q.push(format!("de_continent={de}"));
    }
    q.join("&")
}

/// Percent-encoding for a query value.
fn enc(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

/// One connection's life: the backlog, then the stream until it ends
/// (Ok) or fails.
async fn follow(dx: &Dx, c: &DxSpots, rf_lo: f64, rf_hi: f64) -> Result<(), String> {
    let base = Base::parse(&c.server)?;
    let tls = if base.tls { Some(connector(c)?) } else { None };
    let max_age = c.max_age_minutes.clamp(1, 60) as f64 * 60.0;
    let f = filters(c);
    let amp = if f.is_empty() { "" } else { "&" };
    let take = |v: &serde_json::Value| {
        if let Some(s) = DxSpot::from_json(v)
            && s.hz >= rf_lo
            && s.hz <= rf_hi
        {
            dx.add(s);
        }
    };
    // what was spotted lately
    let path = format!("/spots?max_age={}{amp}{f}", max_age as u64);
    let r = base.get(tls.as_ref(), &path, "application/json").await?;
    let body = r.body.all(MAX_BODY).await?;
    let list: serde_json::Value =
        serde_json::from_slice(&body).map_err(|e| format!("spots: {e}"))?;
    for v in list.as_array().into_iter().flatten() {
        take(v);
    }
    dx.prune(unix_now() - max_age);
    // then each new one
    let path = format!("/spots/stream{}{f}", if f.is_empty() { "" } else { "?" });
    let mut r = base.get(tls.as_ref(), &path, "text/event-stream").await?;
    dx.connected.store(true, Ordering::Relaxed);
    let mut sse = Sse::default();
    let mut pruned = Instant::now();
    loop {
        let chunk = match tokio::time::timeout(IDLE, r.body.chunk()).await {
            Err(_) => return Ok(()), // quiet too long: open it again
            Ok(r) => r?,
        };
        let Some(chunk) = chunk else {
            return Ok(());
        };
        for data in sse.feed(&chunk) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
                take(&v);
            }
        }
        if pruned.elapsed() > Duration::from_secs(60) {
            dx.prune(unix_now() - max_age);
            pruned = Instant::now();
        }
    }
}

/// The CA roots: `ca_file`, else the system's bundle, else the built-in.
fn roots(c: &DxSpots) -> Result<RootCertStore, String> {
    let pem: Vec<u8> = if !c.ca_file.as_os_str().is_empty() {
        std::fs::read(&c.ca_file).map_err(|e| format!("{}: {e}", c.ca_file.display()))?
    } else {
        SYSTEM_ROOTS
            .iter()
            .find_map(|p| std::fs::read(p).ok())
            .unwrap_or_else(|| BUILTIN_ROOTS.to_vec())
    };
    let mut store = RootCertStore::empty();
    for cert in CertificateDer::pem_slice_iter(&pem).flatten() {
        let _ = store.add(cert);
    }
    if store.is_empty() {
        return Err("no CA certificates".into());
    }
    Ok(store)
}

fn connector(c: &DxSpots) -> Result<tokio_rustls::TlsConnector, String> {
    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    let cfg = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots(c)?)
        .with_no_client_auth();
    Ok(tokio_rustls::TlsConnector::from(Arc::new(cfg)))
}

// ------------------------------------------------------------------ HTTP

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// `http(s)://host[:port]/prefix`.
#[derive(Debug, PartialEq)]
struct Base {
    tls: bool,
    host: String,
    port: u16,
    prefix: String,
}

impl Base {
    fn parse(url: &str) -> Result<Base, String> {
        let (tls, rest) = if let Some(r) = url.strip_prefix("https://") {
            (true, r)
        } else if let Some(r) = url.strip_prefix("http://") {
            (false, r)
        } else {
            return Err(format!("{url}: not an http(s) address"));
        };
        let (authority, prefix) = match rest.find('/') {
            Some(k) => (&rest[..k], rest[k..].trim_end_matches('/')),
            None => (rest, ""),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) if !h.ends_with(']') || authority.starts_with('[') => {
                match p.parse::<u16>() {
                    Ok(p) => (h, p),
                    Err(_) => (authority, if tls { 443 } else { 80 }),
                }
            }
            _ => (authority, if tls { 443 } else { 80 }),
        };
        let host = host.trim_start_matches('[').trim_end_matches(']');
        if host.is_empty() {
            return Err(format!("{url}: no host"));
        }
        Ok(Base {
            tls,
            host: host.to_string(),
            port,
            prefix: prefix.to_string(),
        })
    }

    async fn get(
        &self,
        tls: Option<&tokio_rustls::TlsConnector>,
        path: &str,
        accept: &str,
    ) -> Result<Response, String> {
        let tcp = tokio::time::timeout(
            CONNECT_TIMEOUT,
            tokio::net::TcpStream::connect((self.host.as_str(), self.port)),
        )
        .await
        .map_err(|_| "connect: timed out".to_string())?
        .map_err(|e| format!("connect: {e}"))?;
        let mut io: Box<dyn Io> = match tls {
            Some(t) => {
                let name = ServerName::try_from(self.host.clone()).map_err(|e| e.to_string())?;
                Box::new(
                    tokio::time::timeout(CONNECT_TIMEOUT, t.connect(name, tcp))
                        .await
                        .map_err(|_| "TLS: timed out".to_string())?
                        .map_err(|e| format!("TLS: {e}"))?,
                )
            }
            None => Box::new(tcp),
        };
        let host = if (self.tls && self.port == 443) || (!self.tls && self.port == 80) {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        };
        let req = format!(
            "GET {}{path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: web888d/{} (+https://github.com/jeskko/web888-ng)\r\n\
             Accept: {accept}\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n",
            self.prefix,
            env!("CARGO_PKG_VERSION"),
        );
        io.write_all(req.as_bytes())
            .await
            .map_err(|e| format!("send: {e}"))?;
        let r = tokio::time::timeout(CONNECT_TIMEOUT, Response::read(io))
            .await
            .map_err(|_| "no answer".to_string())??;
        if r.status != 200 {
            return Err(format!("{path}: HTTP {}", r.status));
        }
        Ok(r)
    }
}

struct Response {
    status: u16,
    body: Body,
}

impl Response {
    async fn read(mut io: Box<dyn Io>) -> Result<Response, String> {
        let mut buf = Vec::new();
        let end = loop {
            if let Some(k) = find(&buf, b"\r\n\r\n") {
                break k;
            }
            if buf.len() > 64 << 10 {
                return Err("headers too long".into());
            }
            let mut b = [0u8; 4096];
            let n = io.read(&mut b).await.map_err(|e| format!("read: {e}"))?;
            if n == 0 {
                return Err("closed before an answer".into());
            }
            buf.extend_from_slice(&b[..n]);
        };
        let head = String::from_utf8_lossy(&buf[..end]).to_string();
        let rest = buf[end + 4..].to_vec();
        let mut lines = head.split("\r\n");
        let status = lines
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse::<u16>().ok())
            .ok_or("not an HTTP answer")?;
        let mut framing = Framing::Close;
        for l in lines {
            let Some((k, v)) = l.split_once(':') else {
                continue;
            };
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
            if k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked") {
                framing = Framing::Chunked;
            } else if k == "content-length"
                && !matches!(framing, Framing::Chunked)
                && let Ok(n) = v.parse()
            {
                framing = Framing::Length(n);
            }
        }
        Ok(Response {
            status,
            body: Body {
                io,
                buf: rest,
                framing,
                left: 0,
                done: false,
            },
        })
    }
}

#[derive(Clone, Copy)]
enum Framing {
    Length(usize),
    Chunked,
    Close,
}

/// A response body, read as it comes.
struct Body {
    io: Box<dyn Io>,
    /// Read, not yet given out.
    buf: Vec<u8>,
    framing: Framing,
    /// Chunked: bytes left of the current chunk.
    left: usize,
    done: bool,
}

impl Body {
    /// More bytes from the socket into `buf`; false at its end.
    async fn fill(&mut self) -> Result<bool, String> {
        let mut b = [0u8; 8192];
        let n = self
            .io
            .read(&mut b)
            .await
            .map_err(|e| format!("read: {e}"))?;
        self.buf.extend_from_slice(&b[..n]);
        Ok(n > 0)
    }

    /// The next piece of the body; none at its end.
    async fn chunk(&mut self) -> Result<Option<Vec<u8>>, String> {
        if self.done {
            return Ok(None);
        }
        match self.framing {
            Framing::Length(n) => {
                if n == 0 {
                    self.done = true;
                    return Ok(None);
                }
                if self.buf.is_empty() && !self.fill().await? {
                    return Err("body cut short".into());
                }
                let k = self.buf.len().min(n);
                self.framing = Framing::Length(n - k);
                Ok(Some(self.buf.drain(..k).collect()))
            }
            Framing::Close => {
                if self.buf.is_empty() && !self.fill().await? {
                    self.done = true;
                    return Ok(None);
                }
                Ok(Some(std::mem::take(&mut self.buf)))
            }
            Framing::Chunked => {
                if self.left == 0 {
                    // the size line (after the previous chunk's CRLF)
                    let line = loop {
                        if let Some(k) = find(&self.buf, b"\r\n") {
                            let l = String::from_utf8_lossy(&self.buf[..k]).to_string();
                            self.buf.drain(..k + 2);
                            if l.is_empty() {
                                continue; // the CRLF ending a chunk
                            }
                            break l;
                        }
                        if !self.fill().await? {
                            return Err("chunked body cut short".into());
                        }
                    };
                    let size = line.split(';').next().unwrap_or("").trim();
                    let n = usize::from_str_radix(size, 16)
                        .map_err(|_| format!("bad chunk size {size:?}"))?;
                    if n == 0 {
                        self.done = true;
                        return Ok(None);
                    }
                    self.left = n;
                }
                if self.buf.is_empty() && !self.fill().await? {
                    return Err("chunked body cut short".into());
                }
                let k = self.buf.len().min(self.left);
                self.left -= k;
                Ok(Some(self.buf.drain(..k).collect()))
            }
        }
    }

    /// The whole body, at most `max` bytes.
    async fn all(mut self, max: usize) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        while let Some(c) = self.chunk().await? {
            out.extend_from_slice(&c);
            if out.len() > max {
                return Err("answer too big".into());
            }
        }
        Ok(out)
    }
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

/// Server-sent events: the `data` of each event, from the bytes as they
/// come.
#[derive(Default)]
struct Sse {
    line: Vec<u8>,
    data: String,
}

impl Sse {
    fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut out = vec![];
        for &b in bytes {
            if b != b'\n' {
                self.line.push(b);
                continue;
            }
            let mut l = std::mem::take(&mut self.line);
            if l.last() == Some(&b'\r') {
                l.pop();
            }
            let l = String::from_utf8_lossy(&l);
            if l.is_empty() {
                if !self.data.is_empty() {
                    out.push(std::mem::take(&mut self.data));
                }
            } else if let Some(d) = l.strip_prefix("data:") {
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(d.strip_prefix(' ').unwrap_or(d));
            }
            // event:, id:, retry: and comments aren't needed
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spot(id: &str, dx: &str, hz: f64, time: f64) -> DxSpot {
        DxSpot::from_json(&json!({
            "id": id, "dx_call": dx, "de_call": "N0CALL", "freq": hz,
            "band": if hz < 10e6 { "40m" } else { "20m" },
            "mode": "CW", "mode_type": "CW", "time": time,
            "comment": "tnx", "source": "Cluster",
        }))
        .unwrap()
    }

    #[test]
    fn spots_from_spothole_json() {
        let s = DxSpot::from_json(&json!({
            "id": "a1", "dx_call": "KE5AKL", "de_call": "KE5AKL", "freq": 7063000.0,
            "mode": "CW", "band": "40m", "time": 1794507605.5, "comment": "12 X Goat",
            "source": "SOTA", "activities": ["SOTA"],
            "activity_refs": [{"activity": "SOTA", "id": "W5N/NL-004"}],
            "dx_latitude": 36.6, "dx_longitude": -107.8, "dx_location_good": true,
            "de_latitude": 91.0, "de_longitude": 0.0,
        }))
        .unwrap();
        assert_eq!((s.dx_pos, s.de_pos), (Some((36.6, -107.8)), None));
        let rough = DxSpot::from_json(&json!({"dx_call": "X", "freq": 7e6,
            "dx_latitude": 1.0, "dx_longitude": 2.0, "dx_location_good": false}))
        .unwrap();
        assert_eq!(rough.dx_pos, None);
        let rr73 = DxSpot::from_json(&json!({"dx_call": "X", "freq": 14.074e6, "dx_grid": "RR73",
            "dx_latitude": 83.5, "dx_longitude": 175.0, "dx_location_good": true}))
        .unwrap();
        assert_eq!(rr73.dx_pos, None);
        assert_eq!(
            (s.dx.as_str(), s.hz, s.tune()),
            ("KE5AKL", 7.063e6, Tune::Cw)
        );
        let m = s.marker();
        assert_eq!(m.src, "dx");
        assert_eq!(m.note, "12 X Goat · SOTA W5N/NL-004 · de KE5AKL · via SOTA");
        assert_eq!(m.schedule, "spotted 18:20");
        // QRT, or no call or frequency: none
        assert!(DxSpot::from_json(&json!({"dx_call": "X", "freq": 7e6, "qrt": true})).is_none());
        assert!(DxSpot::from_json(&json!({"dx_call": "", "freq": 7e6})).is_none());
        assert!(DxSpot::from_json(&json!({"dx_call": "X"})).is_none());
    }

    #[test]
    fn modes_to_tuning() {
        let t = |mode: &str, ty: &str, hz: f64| {
            DxSpot::from_json(&json!({"dx_call": "X", "freq": hz, "mode": mode, "mode_type": ty}))
                .unwrap()
                .tune()
        };
        assert_eq!(t("SSB", "PHONE", 7.1e6), Tune::Lsb);
        assert_eq!(t("SSB", "PHONE", 14.2e6), Tune::Usb);
        assert_eq!(t("FT8", "DATA", 7.074e6), Tune::Usb);
        assert_eq!(t("FM", "PHONE", 29.6e6), Tune::Nbfm);
        assert_eq!(t("", "CW", 7.0e6), Tune::Cw);
        assert_eq!(t("", "", 3.7e6), Tune::Lsb);
    }

    #[test]
    fn the_latest_spot_per_station_and_band() {
        let dx = Dx::default();
        assert!(dx.add(spot("1", "A1A", 14.020e6, 100.0)));
        assert!(!dx.add(spot("1", "A1A", 14.020e6, 100.0)), "a repeat");
        dx.add(spot("2", "A1A", 14.025e6, 200.0));
        dx.add(spot("3", "A1A", 7.010e6, 150.0));
        dx.add(spot("4", "B2B", 14.010e6, 50.0));
        let m = dx.markers(13e6, 15e6, 0.0);
        let got: Vec<(&str, f64)> = m.iter().map(|m| (m.name.as_str(), m.hz)).collect();
        assert_eq!(got, [("B2B", 14.010e6), ("A1A", 14.025e6)]);
        // too old
        assert_eq!(dx.markers(13e6, 15e6, 60.0).len(), 1);
        assert_eq!(dx.markers(0.0, 30e6, 0.0).len(), 3);
        dx.prune(120.0);
        assert_eq!(dx.len(), 2);
        // a pruned id can come again
        assert!(dx.add(spot("1", "A1A", 14.020e6, 300.0)));
    }

    #[test]
    fn server_sent_events() {
        let mut s = Sse::default();
        assert_eq!(s.feed(b": hello\n\ndata: {\"a\""), Vec::<String>::new());
        assert_eq!(
            s.feed(b":1}\r\n\r\ndata: x\n"),
            vec!["{\"a\":1}".to_string()]
        );
        assert_eq!(s.feed(b"data: y\n\n"), vec!["x\ny".to_string()]);
    }

    #[test]
    fn base_addresses() {
        let b = Base::parse("https://spothole.app/api/v3/").unwrap();
        assert_eq!(
            b,
            Base {
                tls: true,
                host: "spothole.app".into(),
                port: 443,
                prefix: "/api/v3".into()
            }
        );
        let b = Base::parse("http://127.0.0.1:8080").unwrap();
        assert_eq!((b.tls, b.port, b.prefix.as_str()), (false, 8080, ""));
        let b = Base::parse("http://[::1]:81/x").unwrap();
        assert_eq!((b.host.as_str(), b.port), ("::1", 81));
        assert!(Base::parse("ftp://x").is_err());
    }

    #[test]
    fn filters_as_a_query() {
        let mut c = DxSpots::default();
        assert_eq!(filters(&c), "source=Cluster");
        c.sources = vec!["Cluster".into(), "POTA".into()];
        c.de_continents = vec!["EU".into()];
        assert_eq!(filters(&c), "source=Cluster,POTA&de_continent=EU");
        c.sources = vec!["A B".into()];
        c.de_continents.clear();
        assert_eq!(filters(&c), "source=A%20B");
    }

    #[test]
    fn the_builtin_roots_load() {
        let c = DxSpots {
            ca_file: "/no/such/file".into(),
            ..Default::default()
        };
        assert!(roots(&c).is_err());
        let mut s = RootCertStore::empty();
        for cert in CertificateDer::pem_slice_iter(BUILTIN_ROOTS) {
            s.add(cert.unwrap()).unwrap();
        }
        assert_eq!(s.len(), 2);
    }

    /// A fake Spothole on a loopback port: the backlog with a length, the
    /// stream in chunks, then the close.
    #[tokio::test]
    async fn follows_a_server() {
        use tokio::net::TcpListener;
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let now = unix_now();
        let server = tokio::spawn(async move {
            let mut reqs = vec![];
            for k in 0..2 {
                let (mut s, _) = l.accept().await.unwrap();
                let mut buf = vec![0u8; 4096];
                let n = s.read(&mut buf).await.unwrap();
                reqs.push(String::from_utf8_lossy(&buf[..n]).to_string());
                if k == 0 {
                    let body = json!([
                        {"id": "1", "dx_call": "A1A", "freq": 14.02e6, "band": "20m", "time": now},
                        {"id": "2", "dx_call": "B2B", "freq": 99e6, "band": "3m", "time": now},
                    ])
                    .to_string();
                    let r = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    s.write_all(r.as_bytes()).await.unwrap();
                } else {
                    s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n")
                        .await
                        .unwrap();
                    let ev = format!(
                        "data: {}\n\n",
                        json!({"id": "3", "dx_call": "C3C", "freq": 7.01e6, "band": "40m", "time": now})
                    );
                    let (a, b) = ev.split_at(10);
                    for part in [a, b] {
                        s.write_all(format!("{:x}\r\n{part}\r\n", part.len()).as_bytes())
                            .await
                            .unwrap();
                        s.flush().await.unwrap();
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    s.write_all(b"0\r\n\r\n").await.unwrap();
                }
            }
            reqs
        });
        let dx = Dx::default();
        let c = DxSpots {
            server: format!("http://{addr}/api/v3"),
            max_age_minutes: 10,
            ..Default::default()
        };
        follow(&dx, &c, 0.0, 30e6).await.unwrap();
        let reqs = server.await.unwrap();
        assert!(
            reqs[0].starts_with("GET /api/v3/spots?max_age=600&source=Cluster HTTP/1.1\r\n"),
            "{}",
            reqs[0]
        );
        assert!(
            reqs[1].starts_with("GET /api/v3/spots/stream?source=Cluster HTTP/1.1\r\n"),
            "{}",
            reqs[1]
        );
        assert!(reqs[1].contains("User-Agent: web888d/"));
        // out of range (99 MHz) left out
        let got: Vec<String> = dx
            .markers(0.0, 30e6, 0.0)
            .into_iter()
            .map(|m| m.name)
            .collect();
        assert_eq!(got, ["C3C", "A1A"]);
        assert_eq!(dx.received.load(Ordering::Relaxed), 2);
    }
}
