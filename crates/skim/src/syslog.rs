// SPDX-License-Identifier: GPL-3.0-or-later
//! The skimmers' log to a syslog receiver (`doc/m4-skimmers.md` §6.6):
//! band and upload events (`events`) and, if asked, each spot. RFC 5424
//! or RFC 3164 messages, over UDP (one per datagram) or TCP (RFC 6587
//! octet counting, reconnecting). Messages that can't go are dropped.

use crate::Spot;
use crate::events::{self, Event, Level};
use std::io::Write;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};
use web888_config::{Syslog, SyslogFormat, SyslogTransport};

const APP: &str = "web888d";
/// Wait before trying a TCP receiver again.
const RECONNECT: Duration = Duration::from_secs(10);

pub fn facility(name: &str) -> Option<u8> {
    const NAMES: [&str; 24] = [
        "kern",
        "user",
        "mail",
        "daemon",
        "auth",
        "syslog",
        "lpr",
        "news",
        "uucp",
        "cron",
        "authpriv",
        "ftp",
        "ntp",
        "security",
        "console",
        "solaris-cron",
        "local0",
        "local1",
        "local2",
        "local3",
        "local4",
        "local5",
        "local6",
        "local7",
    ];
    NAMES
        .iter()
        .position(|n| n.eq_ignore_ascii_case(name))
        .map(|p| p as u8)
}

fn severity(l: Level) -> u8 {
    match l {
        Level::Warning => 4,
        Level::Notice => 5,
        Level::Info => 6,
    }
}

/// One syslog message.
#[allow(clippy::too_many_arguments)]
pub fn format(
    fmt: SyslogFormat,
    facility: u8,
    level: Level,
    time: f64,
    host: &str,
    pid: u32,
    msgid: &str,
    text: &str,
) -> String {
    let pri = facility as u32 * 8 + severity(level) as u32;
    let u = crate::utc::utc(time);
    match fmt {
        SyslogFormat::Rfc5424 => format!(
            "<{pri}>1 {:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z {host} {APP} {pid} {msgid} - {text}",
            u.year, u.month, u.day, u.hour, u.min, u.sec, u.ms
        ),
        SyslogFormat::Rfc3164 => {
            const MON: [&str; 12] = [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
            ];
            format!(
                "<{pri}>{} {:>2} {:02}:{:02}:{:02} {host} {APP}[{pid}]: {msgid}: {text}",
                MON[(u.month - 1) as usize],
                u.day,
                u.hour,
                u.min,
                u.sec
            )
        }
    }
}

fn system_hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: gethostname writes at most `len` bytes into `buf`.
    let r = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if r != 0 {
        return "web888".into();
    }
    let n = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

enum Link {
    Udp(UdpSocket, SocketAddr),
    Tcp(Option<TcpStream>, SocketAddr, Option<Instant>),
}

fn resolve(server: &str) -> std::io::Result<SocketAddr> {
    let with_port = if server
        .rsplit_once(':')
        .is_some_and(|(_, p)| p.parse::<u16>().is_ok())
        && !server.ends_with(']')
    {
        server.to_string()
    } else {
        format!("{server}:514")
    };
    with_port
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::other(format!("{server}: no address")))
}

impl Link {
    fn open(c: &Syslog) -> std::io::Result<Link> {
        let addr = resolve(&c.server)?;
        Ok(match c.transport {
            SyslogTransport::Udp => {
                let bind = if addr.is_ipv4() {
                    "0.0.0.0:0"
                } else {
                    "[::]:0"
                };
                Link::Udp(UdpSocket::bind(bind)?, addr)
            }
            SyslogTransport::Tcp => Link::Tcp(None, addr, None),
        })
    }

    fn send(&mut self, msg: &str) {
        match self {
            Link::Udp(s, a) => {
                let _ = s.send_to(msg.as_bytes(), *a);
            }
            Link::Tcp(conn, addr, retry_at) => {
                if conn.is_none() && retry_at.is_none_or(|t| Instant::now() >= t) {
                    match TcpStream::connect_timeout(addr, Duration::from_secs(5)) {
                        Ok(s) => {
                            s.set_write_timeout(Some(Duration::from_secs(5))).ok();
                            *conn = Some(s);
                        }
                        Err(_) => *retry_at = Some(Instant::now() + RECONNECT),
                    }
                }
                if let Some(s) = conn {
                    let framed = format!("{} {msg}", msg.len());
                    if s.write_all(framed.as_bytes()).is_err() {
                        *conn = None;
                        *retry_at = Some(Instant::now() + RECONNECT);
                    }
                }
            }
        }
    }
}

/// Send the skimmers' log to `config().server` while it is set.
pub fn spawn(
    bus: Arc<crate::SpotBus>,
    config: impl Fn() -> Syslog + Send + 'static,
) -> std::io::Result<()> {
    let mut spots = bus.subscribe();
    let mut evs = events::subscribe();
    let pid = std::process::id();
    std::thread::Builder::new()
        .name("syslog".into())
        .spawn(move || {
            let mut link: Option<(Syslog, Link)> = None;
            let mut warned = String::new();
            loop {
                let c = config();
                if c.server.is_empty() {
                    link = None;
                } else if link.as_ref().is_none_or(|(l, _)| *l != c) {
                    link = match Link::open(&c) {
                        Ok(l) => Some((c.clone(), l)),
                        Err(e) => {
                            let w = format!("syslog: {}: {e}", c.server);
                            if w != warned {
                                eprintln!("{w}");
                                warned = w;
                            }
                            None
                        }
                    };
                }
                let host = if c.hostname.is_empty() {
                    system_hostname()
                } else {
                    c.hostname.clone()
                };
                let fac = facility(&c.facility).unwrap_or(16);
                let mut out: Vec<String> = vec![];
                loop {
                    use tokio::sync::broadcast::error::TryRecvError;
                    match evs.try_recv() {
                        Ok(e) => {
                            let e: &Event = &e;
                            out.push(format(
                                c.format, fac, e.level, e.time, &host, pid, e.source, &e.text,
                            ));
                        }
                        Err(TryRecvError::Lagged(_)) => {}
                        Err(_) => break,
                    }
                }
                loop {
                    use tokio::sync::broadcast::error::TryRecvError;
                    match spots.try_recv() {
                        Ok(s) if c.spots => {
                            let s: &Spot = &s;
                            out.push(format(
                                c.format,
                                fac,
                                Level::Info,
                                crate::unix_now(),
                                &host,
                                pid,
                                "spot",
                                &s.log_line(),
                            ));
                        }
                        Ok(_) | Err(TryRecvError::Lagged(_)) => {}
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Closed) => return,
                    }
                }
                if let Some((_, l)) = &mut link {
                    for m in &out {
                        l.send(m);
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

    #[test]
    fn formats() {
        let t = 1_792_789_905.25;
        assert_eq!(
            format(
                SyslogFormat::Rfc5424,
                16,
                Level::Info,
                t,
                "rx",
                42,
                "spot",
                "FT8 x"
            ),
            "<134>1 2026-10-23T21:11:45.250Z rx web888d 42 spot - FT8 x"
        );
        assert_eq!(
            format(
                SyslogFormat::Rfc3164,
                16,
                Level::Warning,
                t,
                "rx",
                42,
                "wsprnet",
                "down"
            ),
            "<132>Oct 23 21:11:45 rx web888d[42]: wsprnet: down"
        );
        assert_eq!(facility("local7"), Some(23));
        assert_eq!(facility("daemon"), Some(3));
        assert_eq!(facility("nope"), None);
    }

    #[test]
    fn sends_spots_and_events_over_udp() {
        let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
        rx.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let addr = rx.local_addr().unwrap().to_string();
        let bus = crate::SpotBus::new(10);
        spawn(bus.clone(), move || Syslog {
            server: addr.clone(),
            hostname: "rx".into(),
            ..Default::default()
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(400));
        events::emit(Level::Notice, "skim", "start ft8 7074.0 khz");
        bus.publish(Spot {
            time: 1_792_789_905.0,
            mode: "FT8",
            dial_hz: 7_074_000.0,
            freq_hz: 7_075_234.0,
            snr_db: -12.0,
            dt_s: 0.1,
            message: "CQ K1ABC FN42".into(),
            sender: Some("K1ABC".into()),
            grid: Some("FN42".into()),
            drift: None,
            dbm: None,
            live: true,
        });
        let mut got = vec![];
        let mut buf = [0u8; 2048];
        while got.len() < 2 {
            let n = rx.recv(&mut buf).unwrap();
            got.push(String::from_utf8_lossy(&buf[..n]).to_string());
        }
        assert!(
            got.iter().any(|m| m.starts_with("<133>1 ")
                && m.contains(" rx web888d ")
                && m.ends_with(" skim - start ft8 7074.0 khz")),
            "{got:?}"
        );
        assert!(
            got.iter()
                .any(|m| m.contains(" spot - FT8 7074.0 kHz 211145 -12  0.1 1234 CQ K1ABC FN42")),
            "{got:?}"
        );
    }

    #[test]
    fn addresses() {
        assert_eq!(resolve("127.0.0.1").unwrap().port(), 514);
        assert_eq!(resolve("127.0.0.1:1514").unwrap().port(), 1514);
        assert_eq!(resolve("[::1]:1514").unwrap().port(), 1514);
    }
}
