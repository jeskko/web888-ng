// SPDX-License-Identifier: GPL-3.0-or-later
//! WSPRnet uploads (`doc/m4-skimmers.md` §6.2): one HTTP GET per WSPR
//! spot to `wsprnet.org/post`, with the fields the stock Web-888 sends
//! (`extensions/wspr/wspr_main.cpp`). Spots that fail are retried a few
//! times. Only live spots with a known sender are sent, and only with a
//! callsign and a locator in `[station]`.

use crate::Spot;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Tries per spot, and the wait between them.
const TRIES: u32 = 3;
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// Spots kept waiting at most (the oldest go first).
const MAX_QUEUED: usize = 500;
const TIMEOUT: Duration = Duration::from_secs(15);

/// What the uploader has done (admin page).
#[derive(Clone, Debug, Default)]
pub struct Status {
    pub enabled: bool,
    pub pending: usize,
    /// Spots WSPRnet answered for, and those given up on.
    pub sent: u64,
    pub failed: u64,
    pub skipped_not_live: u64,
    /// UTC seconds of the last spot sent.
    pub last_sent: Option<u64>,
    pub last_reply: Option<String>,
    pub last_error: Option<String>,
}

/// The receiving station.
pub struct Receiver {
    pub callsign: String,
    pub locator: String,
    pub version: String,
}

/// UTC date (YYMMDD) and time (HHMM) of `t`.
fn date_time(t: f64) -> (String, String) {
    let u = crate::utc::utc(t);
    (
        format!("{:02}{:02}{:02}", u.year % 100, u.month, u.day),
        format!("{:02}{:02}", u.hour, u.min),
    )
}

fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The query string of a spot (as the stock firmware's).
pub fn query(rx: &Receiver, s: &Spot) -> Option<String> {
    let call = s.sender.as_deref()?;
    let (date, time) = date_time(s.time);
    // the receiver's frequency: the band's centre, dial + 1500 Hz
    let rqrg = (s.dial_hz + 1500.0) / 1e6;
    let fields = [
        ("function", "wspr".to_string()),
        ("rcall", rx.callsign.clone()),
        ("rgrid", crate::pskreporter::locator(&rx.locator)),
        ("rqrg", format!("{rqrg:.6}")),
        ("date", date),
        ("time", time),
        ("sig", format!("{:.0}", s.snr_db)),
        ("dt", format!("{:.1}", s.dt_s)),
        ("drift", format!("{}", s.drift.unwrap_or(0.0) as i32)),
        ("tqrg", format!("{:.6}", s.freq_hz / 1e6)),
        ("tcall", call.to_string()),
        ("tgrid", s.grid.clone().unwrap_or_default()),
        ("dbm", format!("{}", s.dbm.unwrap_or(0))),
        ("version", rx.version.clone()),
    ];
    Some(
        fields
            .iter()
            .map(|(k, v)| format!("{k}={}", encode(v)))
            .collect::<Vec<_>>()
            .join("&"),
    )
}

/// GET `url?query` over plain HTTP; the response's status and body.
fn get(url: &str, query: &str) -> Result<(u16, String), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or("only http:// URLs are supported")?;
    let (hostport, path) = rest.split_once('/').map_or((rest, ""), |(h, p)| (h, p));
    let host = hostport.split(':').next().unwrap_or(hostport);
    let addr = if hostport.contains(':') {
        hostport.to_string()
    } else {
        format!("{hostport}:80")
    };
    let sa = addr
        .to_socket_addrs()
        .map_err(|e| format!("{host}: {e}"))?
        .next()
        .ok_or(format!("{host}: no address"))?;
    let mut c = TcpStream::connect_timeout(&sa, TIMEOUT).map_err(|e| e.to_string())?;
    c.set_read_timeout(Some(TIMEOUT)).ok();
    c.set_write_timeout(Some(TIMEOUT)).ok();
    let req = format!(
        "GET /{path}?{query} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: web888-ng\r\nConnection: close\r\n\r\n"
    );
    c.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut resp = vec![];
    c.take(64 * 1024)
        .read_to_end(&mut resp)
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&resp);
    let code = text
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or("no HTTP status")?;
    let body = text.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    Ok((code, body.trim().to_string()))
}

/// WSPRnet's answer in a line: "1 out of 1 spot(s) added", or the
/// page's text without its tags.
fn reply_text(body: &str) -> String {
    let mut text = String::new();
    let mut tag = false;
    for c in body.chars() {
        match c {
            '<' => tag = true,
            '>' => {
                tag = false;
                text.push(' ');
            }
            _ if !tag => text.push(c),
            _ => {}
        }
    }
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match text.find(" out of ") {
        Some(i) => {
            let start = text[..i]
                .rfind(|c: char| !c.is_ascii_digit())
                .map_or(0, |p| p + 1);
            let end = text[i..].find("added").map_or(text.len(), |e| i + e + 5);
            text[start..end].to_string()
        }
        None => text.chars().take(200).collect(),
    }
}

struct Pending {
    query: String,
    tries: u32,
    not_before: Instant,
}

/// Queues WSPR spots and sends them.
pub struct Uploader {
    url: String,
    queue: VecDeque<Pending>,
    pub status: Status,
}

impl Uploader {
    pub fn new(url: &str) -> Self {
        Uploader {
            url: url.into(),
            queue: VecDeque::new(),
            status: Status::default(),
        }
    }

    pub fn add(&mut self, rx: &Receiver, s: &Spot) {
        if s.mode != "WSPR" {
            return;
        }
        if !s.live {
            self.status.skipped_not_live += 1;
            return;
        }
        let Some(q) = query(rx, s) else { return };
        if self.queue.len() >= MAX_QUEUED {
            self.queue.pop_front();
            self.status.failed += 1;
        }
        self.queue.push_back(Pending {
            query: q,
            tries: 0,
            not_before: Instant::now(),
        });
    }

    /// Send what is due, one at a time.
    pub fn poll(&mut self, now: Instant) {
        while let Some(p) = self.queue.front() {
            if p.not_before > now {
                break;
            }
            match get(&self.url, &p.query) {
                Ok((200, body)) => {
                    self.status.sent += 1;
                    self.status.last_sent = Some(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                    );
                    self.status.last_reply = Some(reply_text(&body));
                    self.status.last_error = None;
                    self.queue.pop_front();
                }
                r => {
                    self.status.last_error = Some(match r {
                        Ok((code, _)) => format!("HTTP {code}"),
                        Err(e) => e,
                    });
                    let p = self.queue.front_mut().unwrap();
                    p.tries += 1;
                    if p.tries >= TRIES {
                        self.status.failed += 1;
                        self.queue.pop_front();
                    } else {
                        p.not_before = now + RETRY_AFTER;
                        break;
                    }
                }
            }
        }
        self.status.pending = self.queue.len();
    }
}

/// Upload WSPR spots from `bus` while `config` says so.
pub fn spawn(
    bus: Arc<crate::SpotBus>,
    config: impl Fn() -> (web888_config::Station, web888_config::Report) + Send + 'static,
    version: &str,
) -> std::io::Result<Arc<Mutex<Status>>> {
    let status = Arc::new(Mutex::new(Status::default()));
    let st = status.clone();
    let version = format!("web888-ng-{version}");
    let mut spots = bus.subscribe();
    std::thread::Builder::new()
        .name("wsprnet".into())
        .spawn(move || {
            let mut up: Option<Uploader> = None;
            loop {
                let (station, report) = config();
                let enabled =
                    report.wsprnet && !station.callsign.is_empty() && station.locator.len() >= 4;
                if !enabled {
                    up = None;
                    st.lock().unwrap().enabled = false;
                } else if up.as_ref().is_none_or(|u| u.url != report.wsprnet_url) {
                    crate::events::emit(
                        crate::events::Level::Notice,
                        "wsprnet",
                        format!(
                            "uploading as {} {} to {}",
                            station.callsign, station.locator, report.wsprnet_url
                        ),
                    );
                    up = Some(Uploader::new(&report.wsprnet_url));
                }
                let rx = Receiver {
                    callsign: station.callsign.clone(),
                    locator: station.locator.clone(),
                    version: version.clone(),
                };
                loop {
                    use tokio::sync::broadcast::error::TryRecvError;
                    match spots.try_recv() {
                        Ok(s) => {
                            if let Some(u) = &mut up {
                                u.add(&rx, &s);
                            }
                        }
                        Err(TryRecvError::Lagged(_)) => {}
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Closed) => return,
                    }
                }
                if let Some(u) = &mut up {
                    let (sent, err) = (u.status.sent, u.status.last_error.clone());
                    u.poll(Instant::now());
                    if u.status.sent != sent {
                        crate::events::emit(
                            crate::events::Level::Info,
                            "wsprnet",
                            format!(
                                "sent {} spots in all; last reply: {}",
                                u.status.sent,
                                u.status.last_reply.as_deref().unwrap_or("")
                            ),
                        );
                    }
                    if u.status.last_error.is_some() && u.status.last_error != err {
                        crate::events::emit(
                            crate::events::Level::Warning,
                            "wsprnet",
                            format!(
                                "upload failed: {} ({} waiting)",
                                u.status.last_error.as_deref().unwrap_or(""),
                                u.queue.len()
                            ),
                        );
                    }
                    let mut s = u.status.clone();
                    s.enabled = true;
                    *st.lock().unwrap() = s;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        })?;
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rx() -> Receiver {
        Receiver {
            callsign: "N0CALL".into(),
            locator: "fn42AA".into(),
            version: "web888-ng-0.0".into(),
        }
    }

    fn spot(live: bool) -> Spot {
        Spot {
            time: 1_792_789_920.0, // 2026-10-23 21:12:00 UTC
            mode: "WSPR",
            dial_hz: 7_038_600.0,
            freq_hz: 7_040_012.3,
            snr_db: -12.4,
            dt_s: 0.24,
            message: "DL1STF JN58 37".into(),
            sender: Some("DL1STF".into()),
            grid: Some("JN58".into()),
            drift: Some(-1.0),
            dbm: Some(37),
            live,
        }
    }

    #[test]
    fn query_has_the_stock_fields() {
        let q = query(&rx(), &spot(true)).unwrap();
        assert_eq!(
            q,
            "function=wspr&rcall=N0CALL&rgrid=FN42aa&rqrg=7.040100&date=261023&time=2112\
             &sig=-12&dt=0.2&drift=-1&tqrg=7.040012&tcall=DL1STF&tgrid=JN58&dbm=37\
             &version=web888-ng-0.0"
        );
        let mut t2 = spot(true);
        t2.sender = Some("PJ4/K1ABC".into());
        t2.grid = None;
        assert!(
            query(&rx(), &t2)
                .unwrap()
                .contains("&tcall=PJ4%2FK1ABC&tgrid=&")
        );
    }

    #[test]
    fn sends_live_spots_and_retries() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/post", l.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut got = vec![];
            for (k, c) in l.incoming().take(2).enumerate() {
                let mut c = c.unwrap();
                let mut req = vec![];
                let mut buf = [0u8; 4096];
                while !req.ends_with(b"\r\n\r\n") {
                    let n = c.read(&mut buf).unwrap();
                    assert!(n > 0);
                    req.extend_from_slice(&buf[..n]);
                }
                got.push(String::from_utf8_lossy(&req).to_string());
                let reply = if k == 0 {
                    "HTTP/1.1 503 Busy\r\nContent-Length: 0\r\n\r\n"
                } else {
                    "HTTP/1.1 200 OK\r\n\r\n1 out of 1 spot(s) added"
                };
                c.write_all(reply.as_bytes()).unwrap();
            }
            got
        });
        let mut u = Uploader::new(&url);
        u.add(&rx(), &spot(false));
        u.add(&rx(), &spot(true));
        assert_eq!(u.status.skipped_not_live, 1);
        let t = Instant::now();
        u.poll(t);
        assert_eq!((u.status.sent, u.status.pending), (0, 1));
        assert_eq!(u.status.last_error.as_deref(), Some("HTTP 503"));
        u.poll(t + RETRY_AFTER);
        assert_eq!((u.status.sent, u.status.pending), (1, 0));
        assert_eq!(
            u.status.last_reply.as_deref(),
            Some("1 out of 1 spot(s) added")
        );
        let got = server.join().unwrap();
        assert!(
            got[1].starts_with("GET /post?function=wspr&rcall=N0CALL&"),
            "{}",
            got[1]
        );
    }

    #[test]
    fn replies() {
        let page = "<html>\n\n<body>\n  1 out of 1 spot(s) added<br>\nProcessing took 106 milliseconds.<br>\n</body>\n\n</html>";
        assert_eq!(reply_text(page), "1 out of 1 spot(s) added");
        assert_eq!(reply_text("<p>Bad request</p>"), "Bad request");
    }

    #[test]
    fn dates() {
        assert_eq!(date_time(0.0), ("700101".into(), "0000".into()));
        assert_eq!(date_time(1_792_789_920.5), ("261023".into(), "2112".into()));
        assert_eq!(date_time(951_782_400.0), ("000229".into(), "0000".into()));
    }
}
