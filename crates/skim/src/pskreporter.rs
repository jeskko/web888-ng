// SPDX-License-Identifier: GPL-3.0-or-later
//! Spots to PSKReporter (`doc/m4-skimmers.md` §6.1): IPFIX over UDP as in
//! https://pskreporter.info/pskdev.html.
//!
//! The layout is the stock Web-888 server's (`extensions/FT8/
//! PSKReporter.cpp`), which the server is known to accept: an options
//! template without scope fields for the receiver, a 7-field sender
//! template (no iMD), one data set per sender record, and the sequence
//! number counting datagrams. Our first layout, the developer page's
//! templates with all records in one set, was silently dropped
//! (2026-10-23). Their test listener can't check either: the bare
//! pskreporter.info domain is behind a web proxy now, and nothing sent
//! to port 14739 shows on the analysis page.
//!
//! One datagram at most every five minutes (from start-up, with jitter,
//! not on the clock), unless it is full. The record templates go in the
//! first three datagrams and then hourly. A callsign is reported once an
//! hour per band. Only live spots are sent, and only with a callsign and
//! locator configured.

use crate::spot::Spot;
use std::collections::HashMap;
use std::net::UdpSocket;
use std::time::{Duration, Instant};

const LINK_RX: u16 = 0x1138;
const LINK_RX_ANT: u16 = 0x1139;
const LINK_TX: u16 = 0x1140;
const ENTERPRISE: [u8; 4] = [0x00, 0x00, 0x76, 0x8F];

/// Template field: enterprise element `id`, length (0xFFFF = string).
fn field(b: &mut Vec<u8>, id: u16, len: u16) {
    b.extend_from_slice(&(0x8000 | id).to_be_bytes());
    b.extend_from_slice(&len.to_be_bytes());
    b.extend_from_slice(&ENTERPRISE);
}

const STRING: u16 = 0xFFFF;

/// The sender template: callsign, frequency, SNR, mode, locator,
/// information source, flowStartSeconds.
fn sender_template(b: &mut Vec<u8>) {
    let start = b.len();
    b.extend_from_slice(&[0x00, 0x02, 0, 0]);
    b.extend_from_slice(&LINK_TX.to_be_bytes());
    b.extend_from_slice(&7u16.to_be_bytes());
    field(b, 1, STRING);
    field(b, 5, 4);
    field(b, 6, 1);
    field(b, 10, STRING);
    field(b, 3, STRING);
    field(b, 11, 1);
    b.extend_from_slice(&[0x00, 0x96, 0x00, 0x04]);
    let len = (b.len() - start) as u16;
    b[start + 2..start + 4].copy_from_slice(&len.to_be_bytes());
}

/// The receiver template (options, no scope fields): callsign, locator,
/// [antenna,] decoding software.
fn receiver_template(b: &mut Vec<u8>, antenna: bool) {
    let start = b.len();
    b.extend_from_slice(&[0x00, 0x03, 0, 0]);
    b.extend_from_slice(&if antenna { LINK_RX_ANT } else { LINK_RX }.to_be_bytes());
    b.extend_from_slice(&if antenna { 4u16 } else { 3 }.to_be_bytes());
    b.extend_from_slice(&[0, 0]); // scope fields
    field(b, 2, STRING);
    field(b, 4, STRING);
    if antenna {
        field(b, 9, STRING);
    }
    field(b, 8, STRING);
    pad4(b, start);
    let len = (b.len() - start) as u16;
    b[start + 2..start + 4].copy_from_slice(&len.to_be_bytes());
}

/// A Maidenhead locator in its usual case (FN42hn); empty if it isn't
/// one.
pub fn locator(s: &str) -> String {
    let b = s.trim().as_bytes();
    let ok = matches!(b.len(), 4 | 6 | 8)
        && b[0].to_ascii_uppercase().is_ascii_uppercase()
        && b[0].to_ascii_uppercase() <= b'R'
        && b[1].to_ascii_uppercase() <= b'R'
        && b[1].is_ascii_alphabetic()
        && b[2].is_ascii_digit()
        && b[3].is_ascii_digit()
        && (b.len() < 6
            || (b[4].is_ascii_alphabetic()
                && b[4].to_ascii_lowercase() <= b'x'
                && b[5].is_ascii_alphabetic()
                && b[5].to_ascii_lowercase() <= b'x'))
        && (b.len() < 8 || (b[6].is_ascii_digit() && b[7].is_ascii_digit()));
    if !ok {
        return String::new();
    }
    let mut out: String = s.trim()[..4].to_ascii_uppercase();
    if b.len() >= 6 {
        out.push_str(&s.trim()[4..6].to_ascii_lowercase());
    }
    if b.len() == 8 {
        out.push_str(&s.trim()[6..8]);
    }
    out
}

/// Records per datagram (about 28 bytes each with their set header).
pub const MAX_RECORDS: usize = 50;
const INTERVAL: Duration = Duration::from_secs(300);
const TEMPLATES_EVERY: Duration = Duration::from_secs(3600);
const REPORT_AGAIN: f64 = 3600.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Receiver {
    pub callsign: String,
    pub locator: String,
    pub software: String,
    pub antenna: String,
}

/// One sender record.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub call: String,
    pub freq_hz: u32,
    pub snr: i8,
    pub mode: String,
    pub locator: String,
    pub time: u32,
}

fn put_str(b: &mut Vec<u8>, s: &str) {
    let s = &s.as_bytes()[..s.len().min(254)];
    b.push(s.len() as u8);
    b.extend_from_slice(s);
}

fn pad4(b: &mut Vec<u8>, from: usize) {
    while !(b.len() - from).is_multiple_of(4) {
        b.push(0);
    }
}

/// Encode a datagram: header, the templates if asked, the receiver
/// record, then one set per report. `seq` counts datagrams, as stock does.
pub fn datagram(
    now: u32,
    seq: u32,
    domain: u32,
    templates: bool,
    rx: &Receiver,
    reports: &[Report],
) -> Vec<u8> {
    let antenna = !rx.antenna.is_empty();
    let mut b = vec![0x00, 0x0A, 0, 0];
    b.extend_from_slice(&now.to_be_bytes());
    b.extend_from_slice(&seq.to_be_bytes());
    b.extend_from_slice(&domain.to_be_bytes());
    if templates {
        sender_template(&mut b);
        receiver_template(&mut b, antenna);
    }
    let set = |b: &mut Vec<u8>, id: u16, body: &dyn Fn(&mut Vec<u8>)| {
        let start = b.len();
        b.extend_from_slice(&id.to_be_bytes());
        b.extend_from_slice(&[0, 0]);
        body(b);
        pad4(b, start);
        let len = (b.len() - start) as u16;
        b[start + 2..start + 4].copy_from_slice(&len.to_be_bytes());
    };
    set(&mut b, if antenna { LINK_RX_ANT } else { LINK_RX }, &|b| {
        put_str(b, &rx.callsign);
        put_str(b, &locator(&rx.locator));
        if antenna {
            put_str(b, &rx.antenna);
        }
        put_str(b, &rx.software);
    });
    for r in reports {
        set(&mut b, LINK_TX, &|b| {
            put_str(b, &r.call);
            b.extend_from_slice(&r.freq_hz.to_be_bytes());
            b.push(r.snr as u8);
            put_str(b, &r.mode);
            put_str(b, &locator(&r.locator));
            b.push(1); // automatically extracted
            b.extend_from_slice(&r.time.to_be_bytes());
        });
    }
    let len = b.len() as u16;
    b[2..4].copy_from_slice(&len.to_be_bytes());
    b
}

/// What the uploader has done (admin page).
#[derive(Clone, Debug, Default)]
pub struct Status {
    pub datagrams: u64,
    pub reports: u64,
    /// Spots not sent: not live, no sender, or reported recently.
    pub skipped_not_live: u64,
    pub skipped_recent: u64,
    pub last_error: Option<String>,
}

/// Batches spots and sends them.
pub struct Uploader {
    sock: UdpSocket,
    server: String,
    domain: u32,
    seq: u32,
    sent: u32,
    templates_at: Option<Instant>,
    next_send: Instant,
    pending: Vec<Report>,
    /// (call, band in MHz) -> last report time
    reported: HashMap<(String, u32), f64>,
    pub status: Status,
}

fn jitter(seed: u32) -> Duration {
    Duration::from_secs((seed % 30) as u64)
}

impl Uploader {
    pub fn new(server: &str) -> std::io::Result<Self> {
        let sock = UdpSocket::bind("0.0.0.0:0")?;
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let mut x = (t.as_nanos() as u64) ^ ((std::process::id() as u64) << 32) | 1;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let domain = x as u32;
        Ok(Uploader {
            sock,
            server: server.into(),
            domain,
            seq: 0,
            sent: 0,
            templates_at: None,
            next_send: Instant::now() + INTERVAL + jitter(domain),
            pending: vec![],
            reported: HashMap::new(),
            status: Status::default(),
        })
    }

    /// Queue a spot (if it should be reported).
    pub fn add(&mut self, s: &Spot) {
        if !s.live {
            self.status.skipped_not_live += 1;
            return;
        }
        let Some(call) = s.sender.as_deref().filter(|c| !c.starts_with('<')) else {
            return;
        };
        let key = (call.to_ascii_uppercase(), (s.freq_hz / 1e6) as u32);
        if self
            .reported
            .get(&key)
            .is_some_and(|&t| s.time - t < REPORT_AGAIN)
        {
            self.status.skipped_recent += 1;
            return;
        }
        self.reported.insert(key, s.time);
        self.reported.retain(|_, &mut t| s.time - t < REPORT_AGAIN);
        self.pending.push(Report {
            call: call.into(),
            freq_hz: s.freq_hz.round() as u32,
            snr: s.snr_db.round().clamp(-128.0, 127.0) as i8,
            mode: s.mode.into(),
            locator: s.grid.clone().unwrap_or_default(),
            time: s.time as u32,
        });
    }

    /// Send what is due: every five minutes, or at once when full.
    pub fn poll(&mut self, rx: &Receiver, now: Instant) {
        while self.pending.len() >= MAX_RECORDS
            || (now >= self.next_send && !self.pending.is_empty())
        {
            let n = self.pending.len().min(MAX_RECORDS);
            let batch: Vec<Report> = self.pending.drain(..n).collect();
            let templates =
                self.sent < 3 || self.templates_at.is_none_or(|t| now - t >= TEMPLATES_EVERY);
            let unix = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as u32;
            let d = datagram(unix, self.seq, self.domain, templates, rx, &batch);
            match self.sock.send_to(&d, &self.server) {
                Ok(_) => {
                    self.status.datagrams += 1;
                    self.status.reports += batch.len() as u64;
                    self.status.last_error = None;
                }
                Err(e) => self.status.last_error = Some(e.to_string()),
            }
            self.seq = self.seq.wrapping_add(1);
            self.sent += 1;
            if templates {
                self.templates_at = Some(now);
            }
            self.next_send = now + INTERVAL + jitter(self.domain.wrapping_add(self.sent));
        }
        if now >= self.next_send {
            self.next_send = now + INTERVAL;
        }
    }
}

/// Upload spots from `bus` while `config` says so. `config` gives the
/// station and report settings now (they change live).
pub fn spawn(
    bus: std::sync::Arc<crate::SpotBus>,
    config: impl Fn() -> (web888_config::Station, web888_config::Report) + Send + 'static,
    version: &str,
) -> std::io::Result<std::sync::Arc<std::sync::Mutex<Status>>> {
    let status = std::sync::Arc::new(std::sync::Mutex::new(Status::default()));
    let st = status.clone();
    let software = format!("web888-ng {version}");
    let mut spots = bus.subscribe();
    std::thread::Builder::new()
        .name("pskreporter".into())
        .spawn(move || {
            let mut up: Option<Uploader> = None;
            loop {
                let (station, report) = config();
                let enabled = report.pskreporter
                    && !station.callsign.is_empty()
                    && station.locator.len() >= 4;
                if !enabled {
                    up = None;
                } else if up
                    .as_ref()
                    .is_none_or(|u| u.server != report.pskreporter_server)
                {
                    match Uploader::new(&report.pskreporter_server) {
                        Ok(u) => {
                            eprintln!(
                                "pskreporter: uploading as {} {} to {}",
                                station.callsign, station.locator, report.pskreporter_server
                            );
                            up = Some(u);
                        }
                        Err(e) => st.lock().unwrap().last_error = Some(e.to_string()),
                    }
                }
                loop {
                    match spots.try_recv() {
                        Ok(s) => {
                            if let Some(u) = &mut up {
                                u.add(&s);
                            }
                        }
                        Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {}
                        Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                        Err(tokio::sync::broadcast::error::TryRecvError::Closed) => return,
                    }
                }
                if let Some(u) = &mut up {
                    let rx = Receiver {
                        callsign: station.callsign.clone(),
                        locator: station.locator.clone(),
                        software: software.clone(),
                        antenna: station.antenna.clone(),
                    };
                    let before = u.status.datagrams;
                    u.poll(&rx, Instant::now());
                    if u.status.datagrams != before {
                        eprintln!(
                            "pskreporter: sent {} reports in all ({} datagrams){}",
                            u.status.reports,
                            u.status.datagrams,
                            u.status
                                .last_error
                                .as_ref()
                                .map(|e| format!(", error: {e}"))
                                .unwrap_or_default()
                        );
                    }
                    *st.lock().unwrap() = u.status.clone();
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
            locator: "FN42AA".into(),
            software: "web888-ng 0.0".into(),
            antenna: "".into(),
        }
    }

    /// Byte for byte what the stock server's code builds for the same
    /// input (its template and emit code compiled into a harness).
    #[test]
    fn same_bytes_as_stock() {
        let stock = "000a00c86adbcdf0000000012468ace00002003c114000078001ffff0000768f800500040000768f800600010000768f800affff0000768f8003ffff0000768f800b00010000768f00960004000300241138000300008002ffff0000768f8004ffff0000768f8008ffff0000768f00001138001c064f483848514c064b5032357262077765622d383838000011400020054b31414243006bf51cf60346543804464e3432016adbcdb40000001140001c05573958595a006bf83cfd0346543800016adbcdc3000000";
        let rx = Receiver {
            callsign: "OH8HQL".into(),
            locator: "KP25RB".into(),
            software: "web-888".into(),
            antenna: "".into(),
        };
        let now = 1_792_790_000;
        let reports = [
            Report {
                call: "K1ABC".into(),
                freq_hz: 7_075_100,
                snr: -10,
                mode: "FT8".into(),
                locator: "FN42".into(),
                time: now - 60,
            },
            Report {
                call: "W9XYZ".into(),
                freq_hz: 7_075_900,
                snr: -3,
                mode: "FT8".into(),
                locator: "".into(),
                time: now - 45,
            },
        ];
        let d = datagram(now, 1, 0x2468_ace0, true, &rx, &reports);
        let hex: String = d.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, stock);
    }

    #[test]
    fn locators() {
        assert_eq!(locator("KP25RB"), "KP25rb");
        assert_eq!(locator("fn42"), "FN42");
        assert_eq!(locator("R-06"), "");
        assert_eq!(locator("ZZ99"), "");
        assert_eq!(locator("JN58td25"), "JN58td25");
    }

    fn spot(call: &str, t: f64, live: bool) -> Spot {
        Spot {
            time: t,
            mode: "FT8",
            dial_hz: 7.074e6,
            freq_hz: 7.0755e6,
            snr_db: -10.0,
            dt_s: 0.1,
            message: format!("CQ {call} FN42"),
            sender: Some(call.into()),
            grid: Some("FN42".into()),
            live,
        }
    }

    /// Local listener: only live, deduplicated spots arrive, five minutes
    /// apart or when full.
    #[test]
    fn sends_live_spots_once() {
        let l = UdpSocket::bind("127.0.0.1:0").unwrap();
        l.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut u = Uploader::new(&l.local_addr().unwrap().to_string()).unwrap();
        u.add(&spot("K1ABC", 1000.0, true));
        u.add(&spot("K1ABC", 1015.0, true)); // again within the hour
        u.add(&spot("W9XYZ", 1015.0, false)); // a recording
        u.add(&spot("<...>", 1015.0, true));
        let t0 = Instant::now();
        u.poll(&rx(), t0);
        assert_eq!(u.status.datagrams, 0, "not before five minutes");
        u.poll(&rx(), t0 + INTERVAL + Duration::from_secs(31));
        assert_eq!(u.status.datagrams, 1);
        assert_eq!(
            (
                u.status.reports,
                u.status.skipped_recent,
                u.status.skipped_not_live
            ),
            (1, 1, 1)
        );
        let mut buf = [0u8; 2000];
        let n = l.recv(&mut buf).unwrap();
        assert!(buf[..n].windows(6).any(|w| w == b"\x05K1ABC"));
        assert!(!buf[..n].windows(6).any(|w| w == b"\x05W9XYZ"));
        // A full batch goes at once.
        for k in 0..MAX_RECORDS {
            u.add(&spot(&format!("K{k}AB"), 2000.0, true));
        }
        u.poll(&rx(), t0 + INTERVAL + Duration::from_secs(40));
        assert_eq!(u.status.datagrams, 2);
    }
}
