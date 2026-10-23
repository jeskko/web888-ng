// SPDX-License-Identifier: GPL-3.0-or-later
//! Spots to PSKReporter (`doc/m4-skimmers.md` §6.1): IPFIX over UDP as in
//! https://pskreporter.info/pskdev.html.
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

/// Receiver: callsign, locator, decoding software, antenna.
const RECEIVER_TEMPLATE: [u8; 44] = [
    0x00, 0x03, 0x00, 0x2C, 0x99, 0x92, 0x00, 0x04, 0x00, 0x01, //
    0x80, 0x02, 0xFF, 0xFF, 0x00, 0x00, 0x76, 0x8F, //
    0x80, 0x04, 0xFF, 0xFF, 0x00, 0x00, 0x76, 0x8F, //
    0x80, 0x08, 0xFF, 0xFF, 0x00, 0x00, 0x76, 0x8F, //
    0x80, 0x09, 0xFF, 0xFF, 0x00, 0x00, 0x76, 0x8F, //
    0x00, 0x00,
];

/// Sender: callsign, frequency, SNR, IMD, mode, information source,
/// locator, flowStartSeconds.
const SENDER_TEMPLATE: [u8; 68] = [
    0x00, 0x02, 0x00, 0x44, 0x99, 0x93, 0x00, 0x08, //
    0x80, 0x01, 0xFF, 0xFF, 0x00, 0x00, 0x76, 0x8F, //
    0x80, 0x05, 0x00, 0x04, 0x00, 0x00, 0x76, 0x8F, //
    0x80, 0x06, 0x00, 0x01, 0x00, 0x00, 0x76, 0x8F, //
    0x80, 0x07, 0x00, 0x01, 0x00, 0x00, 0x76, 0x8F, //
    0x80, 0x0A, 0xFF, 0xFF, 0x00, 0x00, 0x76, 0x8F, //
    0x80, 0x0B, 0x00, 0x01, 0x00, 0x00, 0x76, 0x8F, //
    0x80, 0x03, 0xFF, 0xFF, 0x00, 0x00, 0x76, 0x8F, //
    0x00, 0x96, 0x00, 0x04,
];

/// Records per datagram (about 16 bytes each, ~1400 bytes in all).
pub const MAX_RECORDS: usize = 70;
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

/// Encode a datagram.
pub fn datagram(
    now: u32,
    seq: u32,
    domain: u32,
    templates: bool,
    rx: &Receiver,
    reports: &[Report],
) -> Vec<u8> {
    let mut b = vec![0x00, 0x0A, 0, 0];
    b.extend_from_slice(&now.to_be_bytes());
    b.extend_from_slice(&seq.to_be_bytes());
    b.extend_from_slice(&domain.to_be_bytes());
    if templates {
        b.extend_from_slice(&RECEIVER_TEMPLATE);
        b.extend_from_slice(&SENDER_TEMPLATE);
    }
    let set = |b: &mut Vec<u8>, id: [u8; 2], body: &dyn Fn(&mut Vec<u8>)| {
        let start = b.len();
        b.extend_from_slice(&id);
        b.extend_from_slice(&[0, 0]);
        body(b);
        pad4(b, start);
        let len = (b.len() - start) as u16;
        b[start + 2..start + 4].copy_from_slice(&len.to_be_bytes());
    };
    set(&mut b, [0x99, 0x92], &|b| {
        put_str(b, &rx.callsign);
        put_str(b, &rx.locator);
        put_str(b, &rx.software);
        put_str(b, &rx.antenna);
    });
    if !reports.is_empty() {
        set(&mut b, [0x99, 0x93], &|b| {
            for r in reports {
                put_str(b, &r.call);
                b.extend_from_slice(&r.freq_hz.to_be_bytes());
                b.push(r.snr as u8);
                b.push(0); // IMD
                put_str(b, &r.mode);
                b.push(1); // automatically extracted
                put_str(b, &r.locator);
                b.extend_from_slice(&r.time.to_be_bytes());
            }
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
            self.seq = self.seq.wrapping_add(batch.len() as u32);
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

    #[test]
    fn datagram_layout() {
        let r = Report {
            call: "K1ABC".into(),
            freq_hz: 7_075_123,
            snr: -12,
            mode: "FT8".into(),
            locator: "FN42".into(),
            time: 1_700_000_000,
        };
        let d = datagram(1_700_000_100, 5, 0x1234_5678, true, &rx(), &[r]);
        assert_eq!(&d[..2], &[0x00, 0x0A]);
        assert_eq!(u16::from_be_bytes([d[2], d[3]]) as usize, d.len());
        assert_eq!(&d[12..16], &0x1234_5678u32.to_be_bytes());
        assert_eq!(&d[16..60], &RECEIVER_TEMPLATE);
        assert_eq!(&d[60..128], &SENDER_TEMPLATE);
        // Receiver set: 99 92, length, then N0CALL FN42AA ... padded.
        assert_eq!(&d[128..130], &[0x99, 0x92]);
        let rl = u16::from_be_bytes([d[130], d[131]]) as usize;
        assert_eq!(rl % 4, 0);
        assert_eq!(&d[132..139], b"\x06N0CALL");
        // Sender set.
        let s = 128 + rl;
        assert_eq!(&d[s..s + 2], &[0x99, 0x93]);
        let sl = u16::from_be_bytes([d[s + 2], d[s + 3]]) as usize;
        assert_eq!(s + sl, d.len());
        let rec = &d[s + 4..];
        assert_eq!(&rec[..6], b"\x05K1ABC");
        assert_eq!(&rec[6..10], &7_075_123u32.to_be_bytes());
        assert_eq!(rec[10] as i8, -12);
        assert_eq!(&rec[12..16], b"\x03FT8");
        assert_eq!(rec[16], 1);
        assert_eq!(&rec[17..22], b"\x04FN42");
        assert_eq!(&rec[22..26], &1_700_000_000u32.to_be_bytes());
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
