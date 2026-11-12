// SPDX-License-Identifier: GPL-3.0-or-later
//! One opt-in, bounded Spothole v3 snapshot per receiver, never the spot-reporting bus.
use crate::session::Shared;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use reqwest::{Client, Url, header};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::watch;
use tokio::time::Instant;
use web888_config::{Config, Spothole};

const PERIOD: Duration = Duration::from_secs(60);
const MAX_AGE: f64 = 1800.0;
const MAX_SPOTS: usize = 500;
const MAX_BYTES: usize = 2 * 1024 * 1024;
const FIELDS: &str = "id,dx_call,freq,mode,time,comment,activities,activity_refs,source,qrt";

#[derive(Clone, Debug, Serialize)]
pub struct ActivityRef {
    pub activity: String,
    pub id: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ExternalSpot {
    pub id: String,
    pub t: f64,
    pub mode: String,
    pub reported_mode: String,
    pub band: String,
    pub hz: f64,
    pub dial: f64,
    pub call: String,
    pub msg: String,
    pub source: &'static str,
    pub upstream_source: String,
    pub activities: Vec<String>,
    pub refs: Vec<ActivityRef>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub enabled: bool,
    pub state: &'static str,
    pub last_success: Option<f64>,
    pub error: Option<String>,
    pub spots: Vec<ExternalSpot>,
}

#[derive(Default)]
struct Cache {
    config: Option<Spothole>,
    last_success: Option<f64>,
    error: Option<String>,
    spots: Vec<ExternalSpot>,
}

#[derive(Default)]
pub struct ExternalSpots(Mutex<Cache>);

impl ExternalSpots {
    pub fn snapshot(&self, cfg: &Spothole, now: f64) -> Snapshot {
        let c = self.0.lock().unwrap();
        let same = c.config.as_ref() == Some(cfg);
        let last_success = same.then_some(c.last_success).flatten();
        let error = if same { c.error.clone() } else { None };
        Snapshot {
            enabled: cfg.enabled,
            state: if !cfg.enabled {
                "disabled"
            } else if error.is_some() {
                if last_success.is_some() {
                    "stale"
                } else {
                    "unavailable"
                }
            } else if last_success.is_some() {
                "ready"
            } else {
                "loading"
            },
            last_success,
            error,
            spots: if cfg.enabled && same {
                c.spots
                    .iter()
                    .filter(|s| s.t >= now - MAX_AGE)
                    .cloned()
                    .collect()
            } else {
                Vec::new()
            },
        }
    }

    fn reset(&self, cfg: &Spothole) {
        *self.0.lock().unwrap() = Cache {
            config: Some(cfg.clone()),
            ..Default::default()
        };
    }

    fn update(&self, result: Result<Vec<ExternalSpot>, FetchError>, now: f64) -> Option<Duration> {
        let mut c = self.0.lock().unwrap();
        match result {
            Ok(spots) => {
                c.spots = spots;
                c.last_success = Some(now);
                c.error = None;
                None
            }
            Err(e) => {
                c.error = Some(e.message);
                c.spots.retain(|s| s.t >= now - MAX_AGE);
                Some(e.retry_after.unwrap_or(PERIOD))
            }
        }
    }
}

pub async fn get(State(sh): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    if sh.live.borrow().auth.listener_hash.is_some() && crate::admin::role(&sh, &headers).is_none()
    {
        return (StatusCode::UNAUTHORIZED, "login required").into_response();
    }
    Json(
        sh.external_spots
            .snapshot(&settings(&sh.live.borrow()), now()),
    )
    .into_response()
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// Reuse the station's configured display identity unless explicitly overridden.
fn settings(config: &Config) -> Spothole {
    let mut feed = config.spots.spothole.clone();
    if feed.station_name.trim().is_empty() {
        feed.station_name = if !config.kiwi.name.trim().is_empty() {
            config.kiwi.name.trim().into()
        } else if !config.station.callsign.trim().is_empty() {
            format!("web888d {}", config.station.callsign.trim())
        } else {
            "web888d".into()
        };
    }
    feed
}

fn endpoint(
    cfg: &Spothole,
) -> Result<(Url, header::HeaderValue, header::HeaderValue), &'static str> {
    let mut url = Url::parse(&cfg.url).map_err(|_| "Invalid Spothole HTTPS URL")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Spothole URL must be HTTPS without credentials, query or fragment");
    }
    let station =
        Url::parse(&cfg.station_url).map_err(|_| "Set the public station name and URL")?;
    let host = station.host_str().ok_or("Set the public station URL")?;
    if !matches!(station.scheme(), "http" | "https")
        || !station.username().is_empty()
        || station.password().is_some()
        || station.fragment().is_some()
        || host == "localhost"
        || host.ends_with(".local")
        || host.ends_with(".localhost")
        || host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| match ip {
                std::net::IpAddr::V4(ip) => {
                    ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified()
                }
                std::net::IpAddr::V6(ip) => {
                    ip.is_loopback()
                        || ip.is_unique_local()
                        || ip.is_unicast_link_local()
                        || ip.is_unspecified()
                }
            })
    {
        return Err("Station URL must identify the public receiver, not a private address");
    }
    let name = cfg.station_name.trim();
    if name.is_empty() || name.len() > 120 || name.chars().any(char::is_control) {
        return Err("Set a public station name (1–120 bytes, no control characters)");
    }
    let agent = header::HeaderValue::from_str(&format!(
        "web888d/{} (https://github.com/jeskko/web888-ng) {name} ({station})",
        env!("CARGO_PKG_VERSION")
    ))
    .map_err(|_| "Invalid public station identity")?;
    let referer = header::HeaderValue::from_str(station.as_str())
        .map_err(|_| "Invalid public station URL")?;
    url.set_path(&format!(
        "{}/api/v3/spots",
        url.path().trim_end_matches('/')
    ));
    url.query_pairs_mut()
        .append_pair("limit", "500")
        .append_pair("max_age", "1800")
        .append_pair("fields", FIELDS);
    Ok((url, agent, referer))
}

#[derive(Debug)]
struct FetchError {
    message: String,
    retry_after: Option<Duration>,
}

impl From<&str> for FetchError {
    fn from(message: &str) -> Self {
        Self {
            message: message.into(),
            retry_after: None,
        }
    }
}

fn retry_delay(value: &str, now: SystemTime) -> Option<Duration> {
    let seconds = value.parse::<u64>().ok().or_else(|| {
        httpdate::parse_http_date(value)
            .ok()?
            .duration_since(now)
            .ok()
            .map(|d| d.as_secs())
    })?;
    Some(Duration::from_secs(seconds.clamp(60, 3600)))
}

/// Configuration validation shared with the admin form. Invalid settings never fetch.
pub(crate) fn validate(config: &Config) -> Result<(), &'static str> {
    let config = settings(config);
    if config.enabled {
        endpoint(&config)?;
    }
    Ok(())
}

async fn fetch(
    client: &Client,
    url: Url,
    agent: header::HeaderValue,
    referer: header::HeaderValue,
    rf: (f64, f64),
) -> Result<Vec<ExternalSpot>, FetchError> {
    let mut response = client
        .get(url)
        .header(header::USER_AGENT, agent)
        .header(header::REFERER, referer)
        .send()
        .await
        .map_err(|_| FetchError::from("Spothole request failed or timed out"))?;
    if !response.status().is_success() {
        let retry_after = response
            .headers()
            .get(header::RETRY_AFTER)
            .and_then(|h| h.to_str().ok())
            .and_then(|s| retry_delay(s, SystemTime::now()));
        return Err(FetchError {
            message: format!("Spothole returned HTTP {}", response.status().as_u16()),
            retry_after,
        });
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BYTES as u64)
    {
        return Err("Spothole response exceeds 2 MiB".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| FetchError::from("Spothole response interrupted"))?
    {
        if body.len() + chunk.len() > MAX_BYTES {
            return Err("Spothole response exceeds 2 MiB".into());
        }
        body.extend_from_slice(&chunk);
    }
    parse(&body, now(), rf)
}

fn text(v: &Value, key: &str, max: usize) -> Option<String> {
    let s = v.get(key)?.as_str()?.trim();
    if s.is_empty() || s.len() > max || s.chars().any(char::is_control) {
        None
    } else {
        Some(s.into())
    }
}

fn parse(body: &[u8], now: f64, rf: (f64, f64)) -> Result<Vec<ExternalSpot>, FetchError> {
    let values: Vec<Value> = serde_json::from_slice(body)
        .map_err(|_| FetchError::from("Invalid Spothole JSON array"))?;
    let mut seen = HashSet::new();
    let mut spots: Vec<_> = values
        .iter()
        .filter_map(|v| {
            let t = v.get("time")?.as_f64()?;
            let hz = v.get("freq")?.as_f64()?;
            if !t.is_finite()
                || !hz.is_finite()
                || t < now - MAX_AGE
                || t > now + 300.0
                || hz < rf.0
                || hz > rf.1
                || v.get("qrt").and_then(Value::as_bool) == Some(true)
            {
                return None;
            }
            let id = text(v, "id", 160)?;
            let call = text(v, "dx_call", 40)?;
            let reported_mode = text(v, "mode", 16)?.to_ascii_uppercase();
            let mode = match reported_mode.as_str() {
                "SSB" if hz < 10_000_000.0 && !(5_000_000.0..6_000_000.0).contains(&hz) => "LSB",
                "SSB" => "USB",
                "CW" => "CW",
                "USB" => "USB",
                "LSB" => "LSB",
                "FT8" => "FT8",
                "FT4" => "FT4",
                "WSPR" => "WSPR",
                "AM" => "AM",
                "FM" => "FM",
                _ => return None,
            }
            .to_string();
            let activities = v
                .get("activities")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|a| a.as_str())
                .filter(|a| !a.is_empty() && a.len() <= 40 && !a.chars().any(char::is_control))
                .take(16)
                .map(String::from)
                .collect();
            let refs = v
                .get("activity_refs")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .take(16)
                .filter_map(|r| {
                    Some(ActivityRef {
                        activity: text(r, "activity", 40)?,
                        id: text(r, "id", 80)?,
                    })
                })
                .collect();
            Some(ExternalSpot {
                id,
                t,
                mode,
                reported_mode,
                band: if (144e6..=148e6).contains(&hz) {
                    "2m"
                } else {
                    web888_skim::spot::band_name(hz)
                }
                .into(),
                hz,
                dial: hz,
                call: call.clone(),
                msg: text(v, "comment", 512).unwrap_or(call),
                source: "spothole",
                upstream_source: text(v, "source", 40).unwrap_or_default(),
                activities,
                refs,
            })
        })
        .collect();
    spots.sort_by(|a, b| b.t.total_cmp(&a.t).then(a.id.cmp(&b.id)));
    spots.retain(|s| seen.insert(s.id.clone()));
    spots.truncate(MAX_SPOTS);
    spots.reverse();
    Ok(spots)
}

/// Spawn once on the web runtime, not once per client or HTTP request.
pub async fn run(cache: Arc<ExternalSpots>, live: watch::Receiver<Arc<Config>>, rf: (f64, f64)) {
    let client = match Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(_) => {
            let cfg = settings(&live.borrow());
            cache.reset(&cfg);
            cache.update(Err("Could not initialize HTTPS client".into()), now());
            return;
        }
    };
    run_with_client(cache, live, rf, client).await;
}

async fn run_with_client(
    cache: Arc<ExternalSpots>,
    mut live: watch::Receiver<Arc<Config>>,
    rf: (f64, f64),
    client: Client,
) {
    let mut active = None;
    let mut next = Instant::now();
    let mut backoff = PERIOD;
    loop {
        let cfg = settings(&live.borrow_and_update());
        if active.as_ref() != Some(&cfg) {
            cache.reset(&cfg);
            active = Some(cfg.clone());
            next = Instant::now();
            backoff = PERIOD;
        }
        if !cfg.enabled {
            if live.changed().await.is_err() {
                break;
            }
            continue;
        }
        let (url, agent, referer) = match endpoint(&cfg) {
            Ok(e) => e,
            Err(e) => {
                cache.update(Err(e.into()), now());
                if live.changed().await.is_err() {
                    break;
                }
                continue;
            }
        };
        if Instant::now() < next {
            tokio::select! {
                r = live.changed() => { if r.is_err() { break; } },
                _ = tokio::time::sleep_until(next) => {},
            }
            continue;
        }
        // Mark the slot before requesting: unrelated configuration reloads cannot
        // cancel a request and immediately multiply its replacement requests.
        next = Instant::now() + PERIOD;
        let result = tokio::select! {
            biased;
            r = live.changed() => { if r.is_err() { break; } continue; },
            result = fetch(&client, url, agent, referer, rf) => result,
        };
        if settings(&live.borrow()) != cfg {
            continue;
        }
        if result.is_ok() {
            backoff = PERIOD;
        }
        let retry = cache.update(result, now());
        if let Some(retry) = retry {
            next = Instant::now() + backoff.max(retry);
            backoff = (backoff * 2).min(Duration::from_secs(900));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn config() -> Spothole {
        Spothole {
            enabled: true,
            station_name: "Test receiver OH0TEST".into(),
            station_url: "https://receiver.example/radio/".into(),
            ..Default::default()
        }
    }
    fn row(id: &str, mode: &str, hz: f64, t: f64) -> Value {
        json!({"id":id,"dx_call":"OH0TEST","mode":mode,"freq":hz,"time":t,"activities":["POTA","WWFF"],
            "activity_refs":[{"activity":"POTA","id":"FI-0001"}],"comment":"CQ test","source":"Cluster"})
    }
    fn parse_rows(rows: Vec<Value>) -> Vec<ExternalSpot> {
        parse(&serde_json::to_vec(&rows).unwrap(), 2000.0, (0.0, 61.44e6)).unwrap()
    }

    #[test]
    fn ssb_inference_explicit_sidebands_and_digital_modes() {
        let s = parse_rows(vec![
            row("a", "SSB", 7.15e6, 2000.0),
            row("b", "SSB", 14.2e6, 2000.0),
            row("c", "SSB", 5.354e6, 2000.0),
            row("d", "USB", 7.1e6, 2000.0),
            row("e", "CW", 7.03e6, 2000.0),
            row("f", "LSB", 14.2e6, 2000.0),
            row("g", "FT8", 7.074e6, 2000.0),
        ]);
        let modes: Vec<_> = s.iter().map(|s| (s.id.as_str(), s.mode.as_str())).collect();
        assert!(
            modes.contains(&("a", "LSB"))
                && modes.contains(&("b", "USB"))
                && modes.contains(&("c", "USB"))
        );
        assert!(
            modes.contains(&("d", "USB"))
                && modes.contains(&("e", "CW"))
                && modes.contains(&("f", "LSB"))
        );
        assert!(modes.contains(&("g", "FT8")));
        assert_eq!(s[0].activities, ["POTA", "WWFF"]);
        assert_eq!(s[0].refs[0].id, "FI-0001");
    }

    #[test]
    fn malformed_expired_future_qrt_unknown_and_out_of_range_are_ignored() {
        let mut qrt = row("qrt", "CW", 7e6, 2000.0);
        qrt["qrt"] = json!(true);
        let s = parse_rows(vec![
            row("ok", "CW", 7e6, 2000.0),
            json!({}),
            qrt,
            row("old", "CW", 7e6, 199.0),
            row("future", "CW", 7e6, 2301.0),
            row("vhf", "CW", 144e6, 2000.0),
            row("unknown", "RTTY", 7e6, 2000.0),
        ]);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].id, "ok");
        assert!(parse(b"{}", 2000.0, (0.0, 61e6)).is_err());
    }

    #[test]
    fn bounded_newest_snapshot_and_id_deduplication() {
        let mut rows: Vec<_> = (0..600)
            .map(|i| row(&i.to_string(), "CW", 7e6, 1000.0 + i as f64))
            .collect();
        rows.push(row("599", "CW", 7.1e6, 1800.0));
        let s = parse_rows(rows);
        assert_eq!(s.len(), 500);
        assert_eq!(s.last().unwrap().hz, 7.1e6);
        assert!(s.windows(2).all(|w| w[0].t <= w[1].t));
    }

    #[test]
    fn identity_headers_identify_only_the_configured_public_station() {
        let (url, agent, referer) = endpoint(&config()).unwrap();
        assert_eq!(url.path(), "/api/v3/spots");
        assert!(url.query().unwrap().contains("limit=500"));
        assert!(
            agent
                .to_str()
                .unwrap()
                .contains("Test receiver OH0TEST (https://receiver.example/radio/)")
        );
        assert_eq!(referer, "https://receiver.example/radio/");
        for url in [
            "http://spothole.app",
            "https://user:secret@example.com",
            "https://example.com/?token=secret",
        ] {
            let mut c = config();
            c.url = url.into();
            assert!(endpoint(&c).is_err());
        }
        for url in [
            "http://192.168.88.53:8073",
            "http://localhost",
            "https://web888.local",
            "http://[::1]/",
            "https://[fd00::1]/",
            "http://[fe80::1]/",
            "https://user:pw@receiver.example",
        ] {
            let mut c = config();
            c.station_url = url.into();
            assert!(endpoint(&c).is_err());
        }
        let mut c = config();
        c.station_name = "bad\r\nInjected: header".into();
        assert!(endpoint(&c).is_err());
    }

    #[test]
    fn cache_disable_source_change_expiry_and_failed_refresh() {
        let cache = ExternalSpots::default();
        let mut c = config();
        cache.reset(&c);
        cache.update(Ok(parse_rows(vec![row("a", "CW", 7e6, 2000.0)])), 2000.0);
        assert_eq!(cache.snapshot(&c, 2050.0).state, "ready");
        cache.update(Err("Unavailable".into()), 2100.0);
        assert_eq!(cache.snapshot(&c, 2100.0).state, "stale");
        assert_eq!(cache.snapshot(&c, 3801.0).spots.len(), 0);
        c.url = "https://other.example".into();
        assert!(cache.snapshot(&c, 2100.0).spots.is_empty());
        c.enabled = false;
        let s = cache.snapshot(&c, 2100.0);
        assert_eq!(s.state, "disabled");
        assert!(s.spots.is_empty());
    }

    async fn fixture(
        status: &str,
        extra: &str,
        body: Vec<u8>,
    ) -> (Url, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
            body.len()
        );
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut buf = [0; 1024];
                let n = socket.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
                if request.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.write_all(&body).await;
            String::from_utf8(request).unwrap()
        });
        (
            Url::parse(&format!("http://{addr}/api/v3/spots")).unwrap(),
            task,
        )
    }

    #[tokio::test]
    async fn http_headers_response_limits_and_retry_after() {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(1))
            .build()
            .unwrap();
        let (_, agent, referer) = endpoint(&config()).unwrap();
        let (url, request) = fixture(
            "200 OK",
            "",
            serde_json::to_vec(&vec![row("ok", "CW", 7e6, now())]).unwrap(),
        )
        .await;
        assert_eq!(
            fetch(&client, url, agent.clone(), referer.clone(), (0.0, 61e6))
                .await
                .unwrap()
                .len(),
            1
        );
        let headers = request.await.unwrap();
        assert!(
            headers
                .to_lowercase()
                .contains("referer: https://receiver.example/radio/")
        );
        assert!(headers.contains("Test receiver OH0TEST"));
        let (url, task) =
            fixture("429 Too Many Requests", "Retry-After: 300\r\n", Vec::new()).await;
        assert_eq!(
            fetch(&client, url, agent.clone(), referer.clone(), (0.0, 61e6))
                .await
                .unwrap_err()
                .retry_after,
            Some(Duration::from_secs(300))
        );
        task.await.unwrap();
        let (url, task) = fixture("200 OK", "", vec![b' '; MAX_BYTES + 1]).await;
        assert!(
            fetch(&client, url, agent.clone(), referer.clone(), (0.0, 61e6))
                .await
                .unwrap_err()
                .message
                .contains("2 MiB")
        );
        task.await.unwrap();
        let (url, task) = fixture(
            "302 Found",
            "Location: https://spothole.app/\r\n",
            Vec::new(),
        )
        .await;
        assert!(
            fetch(&client, url, agent, referer, (0.0, 61e6))
                .await
                .unwrap_err()
                .message
                .contains("302")
        );
        task.await.unwrap();
    }

    #[tokio::test]
    async fn disabled_worker_never_fetches_and_closes_with_config_channel() {
        let mut cfg = Config::default();
        cfg.spots.spothole.url = "http://127.0.0.1:1".into();
        let (tx, rx) = watch::channel(Arc::new(cfg));
        let cache = Arc::new(ExternalSpots::default());
        let worker = tokio::spawn(run(cache.clone(), rx, (0.0, 61e6)));
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert!(cache.0.lock().unwrap().error.is_none());
        drop(tx);
        worker.await.unwrap();
    }
}

#[cfg(test)]
mod scheduling_tests {
    use super::*;
    use axum::routing::get;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio_rustls::{TlsAcceptor, rustls};

    #[test]
    fn identity_reuses_existing_station_config_without_inventing_a_public_url() {
        let mut c = Config::default();
        c.station.callsign = "OH0TEST".into();
        assert_eq!(settings(&c).station_name, "web888d OH0TEST");
        assert!(settings(&c).station_url.is_empty());
        c.kiwi.name = "Public test receiver".into();
        assert_eq!(settings(&c).station_name, "Public test receiver");
        c.spots.spothole.station_name = "Explicit override".into();
        assert_eq!(settings(&c).station_name, "Explicit override");
    }

    #[test]
    fn retry_after_seconds_and_http_dates_are_bounded() {
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        assert_eq!(retry_delay("120", now), Some(Duration::from_secs(120)));
        assert_eq!(retry_delay("1", now), Some(PERIOD));
        assert_eq!(retry_delay("99999", now), Some(Duration::from_secs(3600)));
        let date = httpdate::fmt_http_date(now + Duration::from_secs(300));
        assert_eq!(retry_delay(&date, now), Some(Duration::from_secs(300)));
        assert_eq!(retry_delay("invalid", now), None);
    }

    #[test]
    fn supported_vhf_spots_have_a_2m_band() {
        let rows =
            json!([{"id":"vhf","dx_call":"OH0TEST","freq":144174000,"time":2000,"mode":"SSB"}]);
        let parsed = parse(
            &serde_json::to_vec(&rows).unwrap(),
            2000.0,
            (98.304e6, 147.456e6),
        )
        .unwrap();
        assert_eq!(parsed[0].band, "2m");
        assert_eq!(parsed[0].mode, "USB");
    }

    #[tokio::test]
    async fn one_shared_fetch_per_minute_reload_and_disable() {
        let (cert, key) = crate::tls::self_signed().unwrap();
        let server_config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                CertificateDer::pem_slice_iter(cert.as_bytes())
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap(),
                PrivateKeyDer::from_pem_slice(key.as_bytes()).unwrap(),
            )
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let n = requests.clone();
        let app = axum::Router::new().route(
            "/api/v3/spots",
            get(move || {
                let n = n.clone();
                async move {
                    n.fetch_add(1, Ordering::SeqCst);
                    Json(json!([]))
                }
            }),
        );
        let server = tokio::spawn(crate::serve(
            listener,
            app,
            Some(TlsAcceptor::from(Arc::new(server_config))),
        ));
        let client = Client::builder()
            .add_root_certificate(reqwest::Certificate::from_pem(cert.as_bytes()).unwrap())
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();
        let mut cfg = Config::default();
        cfg.spots.spothole = Spothole {
            enabled: true,
            url: format!("https://{addr}"),
            station_name: "Fixture receiver".into(),
            station_url: "https://receiver.example/".into(),
        };
        let (tx, rx) = watch::channel(Arc::new(cfg.clone()));
        let cache = Arc::new(ExternalSpots::default());
        let worker = tokio::spawn(run_with_client(cache.clone(), rx, (0.0, 61e6), client));
        tokio::time::timeout(Duration::from_secs(3), async {
            while cache
                .snapshot(&cfg.spots.spothole, now())
                .last_success
                .is_none()
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        // Serving any number of listeners only clones the shared cache.
        for _ in 0..32 {
            assert_eq!(cache.snapshot(&cfg.spots.spothole, now()).state, "ready");
        }
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        tokio::time::pause();
        cfg.station.antenna = "Unrelated configuration reload".into();
        tx.send_replace(Arc::new(cfg.clone()));
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        tokio::time::advance(Duration::from_secs(59)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::time::resume();
        tokio::time::timeout(Duration::from_secs(3), async {
            while requests.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        cfg.spots.spothole.enabled = false;
        tx.send_replace(Arc::new(cfg.clone()));
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert_eq!(cache.snapshot(&cfg.spots.spothole, now()).state, "disabled");
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(120)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        tokio::time::resume();
        drop(tx);
        worker.await.unwrap();
        server.abort();
    }
}
