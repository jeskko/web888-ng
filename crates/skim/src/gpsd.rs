// SPDX-License-Identifier: GPL-3.0-or-later
//! The station's locator from gpsd (`doc/m4-skimmers.md` §6.5): a client
//! of gpsd's JSON protocol (`?WATCH`), taking the 6-character Maidenhead
//! locator of each fix. A new locator is taken once it has held for a few
//! reports, so a position on a subsquare's edge doesn't flap. Without a
//! fix the last locator stays, and without one at all the configured
//! locator is used.

use crate::events::{self, Level};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const RECONNECT: Duration = Duration::from_secs(10);
/// Reports a new locator must hold for.
const HOLD: u32 = 5;

/// The 6-character locator ("FN42li") of a position, or None outside
/// the globe.
pub fn locator(lat: f64, lon: f64) -> Option<String> {
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }
    // the north pole and the date line belong to the last field
    let x = (lon + 180.0).min(359.999_999);
    let y = (lat + 90.0).min(179.999_999);
    let c = |base: u8, v: f64| (base + v.floor() as u8) as char;
    Some(
        [
            c(b'A', x / 20.0),
            c(b'A', y / 10.0),
            c(b'0', (x % 20.0) / 2.0),
            c(b'0', y % 10.0),
            c(b'a', (x % 2.0) * 12.0),
            c(b'a', (y % 1.0) * 24.0),
        ]
        .iter()
        .collect(),
    )
}

/// What gpsd said last.
#[derive(Clone, Debug, Default)]
pub struct Status {
    pub connected: bool,
    /// gpsd's fix mode: 0/1 none, 2 2D, 3 3D.
    pub mode: u8,
    /// The locator in use (held), and when the last fix came (UTC s).
    pub locator: Option<String>,
    pub last_fix: Option<f64>,
    pub last_error: Option<String>,
}

/// A fix's mode, latitude and longitude from one line of gpsd JSON (a
/// TPV report), if it is one.
pub fn tpv(line: &str) -> Option<(u8, Option<(f64, f64)>)> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("class")?.as_str()? != "TPV" {
        return None;
    }
    let mode = v.get("mode").and_then(|m| m.as_u64()).unwrap_or(0) as u8;
    let pos = match (
        v.get("lat").and_then(|x| x.as_f64()),
        v.get("lon").and_then(|x| x.as_f64()),
    ) {
        (Some(lat), Some(lon)) if mode >= 2 => Some((lat, lon)),
        _ => None,
    };
    Some((mode, pos))
}

/// Takes a new locator once it has held for `HOLD` fixes.
#[derive(Default)]
struct Holder {
    held: Option<String>,
    candidate: Option<(String, u32)>,
}

impl Holder {
    /// A fix's locator; true if the held one changed.
    fn fix(&mut self, loc: String) -> bool {
        if self.held.as_ref() == Some(&loc) {
            self.candidate = None;
            return false;
        }
        let n = match &self.candidate {
            Some((c, n)) if *c == loc => n + 1,
            _ => 1,
        };
        // the first locator is taken at once
        if n >= HOLD || self.held.is_none() {
            self.held = Some(loc);
            self.candidate = None;
            true
        } else {
            self.candidate = Some((loc, n));
            false
        }
    }
}

fn resolve(server: &str) -> std::io::Result<SocketAddr> {
    let has_port = server
        .rsplit_once(':')
        .is_some_and(|(_, p)| p.parse::<u16>().is_ok())
        && !server.ends_with(']');
    let s = if has_port {
        server.to_string()
    } else {
        format!("{server}:2947")
    };
    s.to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::other(format!("{server}: no address")))
}

/// Follow `config()` (the gpsd address; empty: off) and keep the status.
pub fn spawn(config: impl Fn() -> String + Send + 'static) -> std::io::Result<Arc<Mutex<Status>>> {
    let status = Arc::new(Mutex::new(Status::default()));
    let st = status.clone();
    std::thread::Builder::new()
        .name("gpsd".into())
        .spawn(move || {
            let mut holder = Holder::default();
            let mut warned = String::new();
            loop {
                let server = config();
                if server.is_empty() {
                    st.lock().unwrap().connected = false;
                    std::thread::sleep(Duration::from_secs(2));
                    continue;
                }
                let r = follow(&server, &config, &st, &mut holder);
                {
                    let mut s = st.lock().unwrap();
                    s.connected = false;
                    s.mode = 0;
                    if let Err(e) = &r {
                        s.last_error = Some(e.clone());
                    }
                }
                if let Err(e) = r {
                    let w = format!("{server}: {e}");
                    if w != warned {
                        events::emit(Level::Warning, "gpsd", w.clone());
                        warned = w;
                    }
                    std::thread::sleep(RECONNECT);
                }
            }
        })?;
    Ok(status)
}

/// One connection, until it fails or the address changes.
fn follow(
    server: &str,
    config: &impl Fn() -> String,
    st: &Mutex<Status>,
    holder: &mut Holder,
) -> Result<(), String> {
    let addr = resolve(server).map_err(|e| e.to_string())?;
    let mut c =
        TcpStream::connect_timeout(&addr, Duration::from_secs(5)).map_err(|e| e.to_string())?;
    // gpsd reports about once a second; a quiet minute means it's gone
    c.set_read_timeout(Some(Duration::from_secs(60))).ok();
    c.write_all(b"?WATCH={\"enable\":true,\"json\":true};\n")
        .map_err(|e| e.to_string())?;
    {
        let mut s = st.lock().unwrap();
        s.connected = true;
        s.last_error = None;
    }
    let mut lines = BufReader::new(c);
    let mut line = String::new();
    let mut checked = Instant::now();
    loop {
        line.clear();
        match lines.read_line(&mut line) {
            Ok(0) => return Err("gpsd closed the connection".into()),
            Ok(_) => {}
            Err(e) => return Err(e.to_string()),
        }
        if let Some((mode, pos)) = tpv(&line) {
            let mut s = st.lock().unwrap();
            s.mode = mode;
            if let Some(loc) = pos.and_then(|(lat, lon)| locator(lat, lon)) {
                s.last_fix = Some(crate::unix_now());
                if holder.fix(loc) {
                    let l = holder.held.clone().unwrap();
                    s.locator = Some(l.clone());
                    drop(s);
                    events::emit(Level::Notice, "gpsd", format!("locator {l}"));
                }
            }
        }
        if checked.elapsed() > Duration::from_secs(5) {
            checked = Instant::now();
            if config() != server {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locators() {
        // Oulu, and the corners
        assert_eq!(locator(65.0125, 25.4715).as_deref(), Some("KP25ra"));
        assert_eq!(locator(42.3583, -71.0603).as_deref(), Some("FN42li"));
        assert_eq!(locator(-33.8688, 151.2093).as_deref(), Some("QF56od"));
        assert_eq!(locator(90.0, 180.0).as_deref(), Some("RR99xx"));
        assert_eq!(locator(-90.0, -180.0).as_deref(), Some("AA00aa"));
        assert_eq!(locator(91.0, 0.0), None);
    }

    #[test]
    fn reports() {
        let fix = r#"{"class":"TPV","device":"/dev/ttyS1","mode":3,"lat":65.0125,"lon":25.4715,"alt":12.0}"#;
        assert_eq!(tpv(fix), Some((3, Some((65.0125, 25.4715)))));
        let nofix = r#"{"class":"TPV","mode":1}"#;
        assert_eq!(tpv(nofix), Some((1, None)));
        assert_eq!(tpv(r#"{"class":"SKY","satellites":[]}"#), None);
        assert_eq!(tpv("garbage"), None);
    }

    #[test]
    fn a_new_locator_must_hold() {
        let mut h = Holder::default();
        assert!(h.fix("KP25ga".into()), "the first is taken at once");
        for _ in 0..HOLD - 1 {
            assert!(!h.fix("KP25gb".into()));
        }
        // a flap back resets the count
        assert!(!h.fix("KP25ga".into()));
        for _ in 0..HOLD - 1 {
            assert!(!h.fix("KP25gb".into()));
        }
        assert!(h.fix("KP25gb".into()));
        assert_eq!(h.held.as_deref(), Some("KP25gb"));
    }

    #[test]
    fn follows_a_gpsd() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            let mut r = BufReader::new(c.try_clone().unwrap());
            c.write_all(b"{\"class\":\"VERSION\",\"release\":\"3.25\"}\n")
                .unwrap();
            let mut w = String::new();
            r.read_line(&mut w).unwrap();
            assert!(w.starts_with("?WATCH={\"enable\":true"), "{w}");
            for _ in 0..3 {
                c.write_all(b"{\"class\":\"TPV\",\"mode\":3,\"lat\":42.3583,\"lon\":-71.0603}\n")
                    .unwrap();
            }
            std::thread::sleep(Duration::from_secs(3));
        });
        let st = spawn(move || addr.clone()).unwrap();
        let t = Instant::now();
        while st.lock().unwrap().locator.is_none() && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let s = st.lock().unwrap().clone();
        assert_eq!(s.locator.as_deref(), Some("FN42li"));
        assert!(s.connected);
        assert_eq!(s.mode, 3);
    }
}
