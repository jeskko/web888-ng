// SPDX-License-Identifier: GPL-3.0-or-later
//! The skimmers' log to an MQTT broker (`doc/m4-skimmers.md` §6.4): an
//! MQTT 3.1.1 publisher at QoS 0, as the stock firmware's (plain TCP,
//! optional user and password). Spots go to `<topic>/spot/<MODE>`, band
//! and upload events to `<topic>/band` and `<topic>/upload`, all as JSON;
//! `<topic>/status` is `online` while connected (retained, `offline` as
//! the last will). What can't go while the broker is away is dropped.

use crate::Spot;
use crate::events::{self, Event};
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};
use web888_config::{LogEvent, Mqtt};

const KEEPALIVE: u16 = 60;
const RECONNECT: Duration = Duration::from_secs(10);
const TIMEOUT: Duration = Duration::from_secs(5);

/// MQTT's variable-length "remaining length".
fn remaining(mut n: usize, out: &mut Vec<u8>) {
    loop {
        let mut b = (n % 128) as u8;
        n /= 128;
        if n > 0 {
            b |= 0x80;
        }
        out.push(b);
        if n == 0 {
            break;
        }
    }
}

fn string(s: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(&(s.len() as u16).to_be_bytes());
    out.extend_from_slice(s);
}

fn packet(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut p = vec![kind];
    remaining(body.len(), &mut p);
    p.extend_from_slice(body);
    p
}

/// CONNECT, clean session, with a retained last will.
pub fn connect_packet(client_id: &str, user: &str, pass: &str, will: (&str, &str)) -> Vec<u8> {
    let mut b = vec![];
    string(b"MQTT", &mut b);
    b.push(4); // 3.1.1
    let mut flags = 0x02 | 0x04 | 0x20; // clean session, will, will retain
    if !user.is_empty() {
        flags |= 0x80;
        if !pass.is_empty() {
            flags |= 0x40;
        }
    }
    b.push(flags);
    b.extend_from_slice(&KEEPALIVE.to_be_bytes());
    string(client_id.as_bytes(), &mut b);
    string(will.0.as_bytes(), &mut b);
    string(will.1.as_bytes(), &mut b);
    if !user.is_empty() {
        string(user.as_bytes(), &mut b);
        if !pass.is_empty() {
            string(pass.as_bytes(), &mut b);
        }
    }
    packet(0x10, &b)
}

pub fn publish_packet(topic: &str, payload: &[u8], retain: bool) -> Vec<u8> {
    let mut b = vec![];
    string(topic.as_bytes(), &mut b);
    b.extend_from_slice(payload);
    packet(0x30 | retain as u8, &b)
}

/// A JSON string.
fn js(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// A spot as JSON (the fields of the web protocol's spots, and the
/// stock firmware's `timestamp` and `server`).
pub fn spot_json(s: &Spot, server: &str, now: f64) -> String {
    let mut f = vec![
        format!("\"timestamp\":{}", now as u64),
        format!("\"server\":{}", js(server)),
        format!("\"t\":{}", s.time),
        format!("\"mode\":{}", js(s.mode)),
        format!("\"band\":{}", js(crate::spot::band_name(s.dial_hz))),
        format!("\"dial\":{}", s.dial_hz),
        format!("\"hz\":{}", s.freq_hz.round()),
        format!("\"snr\":{}", s.snr_db.round()),
        format!("\"dt\":{:.1}", s.dt_s),
        format!("\"msg\":{}", js(&s.message)),
    ];
    if let Some(c) = &s.sender {
        f.push(format!("\"call\":{}", js(c)));
    }
    if let Some(g) = &s.grid {
        f.push(format!("\"grid\":{}", js(g)));
    }
    if let Some(d) = s.drift {
        f.push(format!("\"drift\":{d}"));
    }
    if let Some(p) = s.dbm {
        f.push(format!("\"dbm\":{p}"));
    }
    f.push(format!("\"live\":{}", s.live));
    format!("{{{}}}", f.join(","))
}

pub fn event_json(e: &Event, server: &str) -> String {
    format!(
        "{{\"timestamp\":{},\"server\":{},\"source\":{},\"level\":{},\"text\":{}}}",
        e.time as u64,
        js(server),
        js(e.source),
        js(&format!("{:?}", e.level).to_lowercase()),
        js(&e.text)
    )
}

/// Topic levels can't hold wildcards or separators from a host name.
fn topic_safe(s: &str) -> String {
    s.chars()
        .map(|c| if matches!(c, '+' | '#' | '/') { '_' } else { c })
        .collect()
}

fn resolve(server: &str) -> std::io::Result<SocketAddr> {
    let has_port = server
        .rsplit_once(':')
        .is_some_and(|(_, p)| p.parse::<u16>().is_ok())
        && !server.ends_with(']');
    let s = if has_port {
        server.to_string()
    } else {
        format!("{server}:1883")
    };
    s.to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::other(format!("{server}: no address")))
}

struct Conn {
    sock: TcpStream,
    last_tx: Instant,
}

impl Conn {
    fn open(c: &Mqtt, id: &str, topic: &str) -> Result<Conn, String> {
        let addr = resolve(&c.server).map_err(|e| e.to_string())?;
        let mut sock = TcpStream::connect_timeout(&addr, TIMEOUT).map_err(|e| e.to_string())?;
        sock.set_read_timeout(Some(TIMEOUT)).ok();
        sock.set_write_timeout(Some(TIMEOUT)).ok();
        let status = format!("{topic}/status");
        sock.write_all(&connect_packet(
            id,
            &c.username,
            &c.password,
            (&status, "offline"),
        ))
        .map_err(|e| e.to_string())?;
        let mut ack = [0u8; 4];
        sock.read_exact(&mut ack)
            .map_err(|e| format!("CONNACK: {e}"))?;
        if ack[0] != 0x20 {
            return Err(format!("not a CONNACK: {:02x}", ack[0]));
        }
        match ack[3] {
            0 => {}
            4 => return Err("bad user name or password".into()),
            5 => return Err("not authorised".into()),
            rc => return Err(format!("refused ({rc})")),
        }
        sock.write_all(&publish_packet(&status, b"online", true))
            .map_err(|e| e.to_string())?;
        sock.set_nonblocking(true).map_err(|e| e.to_string())?;
        Ok(Conn {
            sock,
            last_tx: Instant::now(),
        })
    }

    fn send(&mut self, p: &[u8]) -> std::io::Result<()> {
        self.sock.set_nonblocking(false)?;
        let r = self.sock.write_all(p);
        self.sock.set_nonblocking(true)?;
        self.last_tx = Instant::now();
        r
    }

    /// Keep the connection alive and see that it still is.
    fn tend(&mut self) -> std::io::Result<()> {
        if self.last_tx.elapsed() >= Duration::from_secs(KEEPALIVE as u64 / 2) {
            self.send(&[0xc0, 0])?; // PINGREQ
        }
        let mut buf = [0u8; 256];
        loop {
            match self.sock.read(&mut buf) {
                Ok(0) => return Err(std::io::Error::other("the broker closed the connection")),
                Ok(_) => {} // PINGRESP
                Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(()),
                Err(e) => return Err(e),
            }
        }
    }
}

/// Publish the skimmers' log to `config().server` while it is set.
pub fn spawn(
    bus: Arc<crate::SpotBus>,
    config: impl Fn() -> Mqtt + Send + 'static,
) -> std::io::Result<()> {
    let mut spots = bus.subscribe();
    let mut evs = events::subscribe();
    std::thread::Builder::new()
        .name("mqtt".into())
        .spawn(move || {
            let mut conn: Option<(Mqtt, Conn)> = None;
            let mut retry_at: Option<Instant> = None;
            let mut warned = String::new();
            loop {
                let c = config();
                let host = events::hostname();
                let id = if c.client_id.is_empty() {
                    format!("web888-{host}")
                } else {
                    c.client_id.clone()
                };
                let topic = if c.topic.is_empty() {
                    format!("web888/{}", topic_safe(&host))
                } else {
                    c.topic.trim_end_matches('/').to_string()
                };
                if conn.as_ref().is_some_and(|(k, _)| *k != c) || c.server.is_empty() {
                    if let Some((_, mut k)) = conn.take() {
                        let _ = k.send(&[0xe0, 0]); // DISCONNECT
                    }
                    retry_at = None;
                }
                if conn.is_none()
                    && !c.server.is_empty()
                    && retry_at.is_none_or(|t| Instant::now() >= t)
                {
                    match Conn::open(&c, &id, &topic) {
                        Ok(k) => {
                            eprintln!(
                                "mqtt: connected to {} as {id}, topics {topic}/...",
                                c.server
                            );
                            warned.clear();
                            conn = Some((c.clone(), k));
                        }
                        Err(e) => {
                            let w = format!("mqtt: {}: {e}", c.server);
                            if w != warned {
                                eprintln!("{w}");
                                warned = w;
                            }
                            retry_at = Some(Instant::now() + RECONNECT);
                        }
                    }
                }
                let now = crate::unix_now();
                let mut out: Vec<Vec<u8>> = vec![];
                loop {
                    use tokio::sync::broadcast::error::TryRecvError;
                    match evs.try_recv() {
                        Ok(e) if events::wants(&c.events, &c.modes, e.kind(), None) => {
                            let sub = match e.kind() {
                                LogEvent::Band => "band",
                                _ => "upload",
                            };
                            out.push(publish_packet(
                                &format!("{topic}/{sub}"),
                                event_json(&e, &id).as_bytes(),
                                false,
                            ));
                        }
                        Ok(_) | Err(TryRecvError::Lagged(_)) => {}
                        Err(_) => break,
                    }
                }
                loop {
                    use tokio::sync::broadcast::error::TryRecvError;
                    match spots.try_recv() {
                        Ok(s)
                            if events::wants(&c.events, &c.modes, LogEvent::Spot, Some(s.mode)) =>
                        {
                            out.push(publish_packet(
                                &format!("{topic}/spot/{}", s.mode),
                                spot_json(&s, &id, now).as_bytes(),
                                false,
                            ));
                        }
                        Ok(_) | Err(TryRecvError::Lagged(_)) => {}
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Closed) => return,
                    }
                }
                if let Some((_, k)) = &mut conn {
                    let r = out
                        .iter()
                        .try_for_each(|p| k.send(p))
                        .and_then(|_| k.tend());
                    if let Err(e) = r {
                        eprintln!("mqtt: {}: {e}; reconnecting", c.server);
                        conn = None;
                        retry_at = Some(Instant::now() + RECONNECT);
                    }
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn packets() {
        // CONNECT "c" with will "t/status" = "offline", no login
        let p = connect_packet("c", "", "", ("t/status", "offline"));
        assert_eq!(&p[..2], &[0x10, (p.len() - 2) as u8]);
        assert_eq!(&p[2..10], b"\0\x04MQTT\x04\x26");
        assert_eq!(&p[10..12], &60u16.to_be_bytes());
        assert_eq!(&p[12..15], b"\0\x01c");
        let p = connect_packet("c", "u", "pw", ("t", "x"));
        assert_eq!(p[9], 0x26 | 0xc0);
        assert!(p.ends_with(b"\0\x01u\0\x02pw"));
        assert_eq!(publish_packet("a/b", b"hi", true), b"\x31\x07\0\x03a/bhi");
        let mut r = vec![];
        remaining(321, &mut r);
        assert_eq!(r, [0xc1, 0x02]);
    }

    #[test]
    fn json() {
        let s = Spot {
            time: 1_792_789_905.0,
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
            live: true,
        };
        assert_eq!(
            spot_json(&s, "web888-rx", 1_792_789_920.0),
            "{\"timestamp\":1792789920,\"server\":\"web888-rx\",\"t\":1792789905,\"mode\":\"WSPR\",\
             \"band\":\"40m\",\"dial\":7038600,\"hz\":7040012,\"snr\":-12,\"dt\":0.2,\
             \"msg\":\"DL1STF JN58 37\",\"call\":\"DL1STF\",\"grid\":\"JN58\",\"drift\":-1,\
             \"dbm\":37,\"live\":true}"
        );
        assert_eq!(js("a\"b\\\n"), "\"a\\\"b\\\\\\u000a\"");
        assert_eq!(topic_safe("a+b#c/d"), "a_b_c_d");
    }

    /// A broker that accepts one client and returns what it got.
    fn broker(rc: u8) -> (String, std::thread::JoinHandle<Vec<u8>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let h = std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut got = vec![];
            let mut buf = [0u8; 4096];
            let n = c.read(&mut buf).unwrap();
            got.extend_from_slice(&buf[..n]);
            c.write_all(&[0x20, 2, 0, rc]).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline && !got.windows(9).any(|w| w == b"CQ K1ABC ") {
                match c.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => got.extend_from_slice(&buf[..n]),
                }
            }
            got
        });
        (addr, h)
    }

    #[test]
    fn publishes_to_a_broker() {
        let (addr, h) = broker(0);
        let bus = crate::SpotBus::new(10);
        spawn(bus.clone(), move || Mqtt {
            server: addr.clone(),
            topic: "t".into(),
            client_id: "c".into(),
            modes: vec![web888_config::SkimMode::Ft8],
            ..Default::default()
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(600));
        let spot = |mode: &'static str, msg: &str| Spot {
            time: 1_792_789_905.0,
            mode,
            dial_hz: 7_074_000.0,
            freq_hz: 7_075_234.0,
            snr_db: -12.0,
            dt_s: 0.1,
            message: msg.into(),
            sender: None,
            grid: None,
            drift: None,
            dbm: None,
            live: true,
        };
        bus.publish(spot("FT4", "CQ N0FT4 FN42"));
        bus.publish(spot("FT8", "CQ K1ABC FN42"));
        let got = h.join().unwrap();
        let text = String::from_utf8_lossy(&got);
        assert!(got.starts_with(&connect_packet("c", "", "", ("t/status", "offline"))));
        assert!(text.contains("t/statusonline"), "{text}");
        assert!(text.contains("t/spot/FT8{\"timestamp\""), "{text}");
        assert!(!text.contains("N0FT4"), "FT4 isn't asked for: {text}");
    }
}
