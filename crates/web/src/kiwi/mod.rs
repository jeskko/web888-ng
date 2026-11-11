// SPDX-License-Identifier: GPL-3.0-or-later
//! The KiwiSDR protocol (`doc/m5-kiwi.md`), for kiwirecorder, SuperSDR
//! and other Kiwi clients: an audio stream (`/<n>/SND`) on an RX channel
//! lease, a waterfall stream (`/<n>/W/F`) on the shared waterfall views,
//! and the `/status` and `/VER` pages. Kiwi's own browser UI isn't
//! served; web888d's UI is at `/`.
//!
//! Ported in behaviour from the upstream server (KiwiSDR-derived,
//! RaspSDR/server: `rx/rx_server.cpp`, `rx/rx_cmd.cpp`,
//! `rx/rx_sound*.cpp`, `rx/rx_waterfall.cpp`, `rx/rx_server_ajax.cpp`;
//! LGPL-2.0-or-later, Copyright (c) 2014-2023 John Seamons,
//! ZL4VO/KF6VO), on web888d's sessions, leases and DSP.

// without the feature only the encoders are used (by the DSP workers)
#![cfg_attr(not(feature = "kiwi"), allow(dead_code, unused_imports))]

pub mod adpcm;
#[cfg(feature = "kiwi")]
mod snd;
#[cfg(feature = "kiwi")]
mod wf;
pub mod wire;

use crate::auth::Role;
use crate::proto::RadioInfo;
use crate::session::Shared;
use axum::extract::ws::{Message, WebSocket};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The KiwiSDR version web888d answers as: the protocol level it speaks
/// (clients choose their API by it; 1.329+ has `SET zoom= cf=`).
pub const VERSION_MAJ: u32 = 1;
pub const VERSION_MIN: u32 = 902;
/// `badp` answers.
pub const BADP_OK: u8 = 0;
pub const BADP_TRY_AGAIN: u8 = 1;
pub const BADP_NO_ADMIN_PWD_SET: u8 = 4;
pub const BADP_NO_MULTIPLE_CONNS: u8 = 5;
/// A stream with no message for this long is closed (before the
/// password: the shorter one).
pub const KEEPALIVE: Duration = Duration::from_secs(60);
pub const KEEPALIVE_NO_AUTH: Duration = Duration::from_secs(20);

/// The SND streams open now, by client address and the timestamp in their
/// URL, so a W/F stream can join its SND stream (tag its lines with the
/// audio's sequence, not count as another session).
pub struct Registry {
    snd: Mutex<HashMap<(IpAddr, u64), Arc<AtomicU32>>>,
    /// When the server started (`/status` uptime).
    pub started: std::time::Instant,
}

impl Default for Registry {
    fn default() -> Self {
        Registry {
            snd: Mutex::default(),
            started: std::time::Instant::now(),
        }
    }
}

/// An SND stream's place in the registry, removed when dropped.
pub(crate) struct SndEntry<'a> {
    reg: &'a Registry,
    key: (IpAddr, u64),
    pub seq: Arc<AtomicU32>,
}

impl Registry {
    /// None if the address already has an SND stream with this timestamp.
    pub(crate) fn add_snd(&self, ip: IpAddr, ts: u64) -> Option<SndEntry<'_>> {
        let mut m = self.snd.lock().unwrap();
        if m.contains_key(&(ip, ts)) {
            return None;
        }
        let seq = Arc::new(AtomicU32::new(0));
        m.insert((ip, ts), seq.clone());
        Some(SndEntry {
            reg: self,
            key: (ip, ts),
            seq,
        })
    }

    /// The sequence of the SND stream this W/F stream belongs with.
    pub(crate) fn snd_seq(&self, ip: IpAddr, ts: u64) -> Option<Arc<AtomicU32>> {
        self.snd.lock().unwrap().get(&(ip, ts)).cloned()
    }

    /// SND streams open.
    pub fn streams(&self) -> usize {
        self.snd.lock().unwrap().len()
    }
}

impl Drop for SndEntry<'_> {
    fn drop(&mut self) {
        self.reg.snd.lock().unwrap().remove(&self.key);
    }
}

/// How Kiwi frequencies map to web888d's: Kiwi's baseband starts at 0 Hz
/// and is `bandwidth` wide; web888d's range starts at `rf_lo`, which Kiwi
/// clients get as the frequency offset (as for a converter).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub rf_lo: f64,
    pub bandwidth: f64,
}

impl Geometry {
    pub fn new(r: &RadioInfo) -> Self {
        Geometry {
            rf_lo: r.rf_lo,
            bandwidth: (r.rf_hi - r.rf_lo).max(1.0),
        }
    }

    /// Hz per unit of the waterfall's `start` (the deepest zoom's pixel).
    pub fn hz_per_start(&self) -> f64 {
        self.bandwidth / (wire::WF_WIDTH as f64 * (1u64 << wire::MAX_ZOOM) as f64)
    }

    /// The span at `zoom`.
    pub fn span(&self, zoom: u32) -> f64 {
        self.bandwidth / (1u64 << zoom.min(wire::MAX_ZOOM)) as f64
    }
}

/// The receiver's name, as Kiwi clients show it.
pub(crate) fn rx_name(c: &web888_config::Config) -> String {
    if !c.kiwi.name.is_empty() {
        return c.kiwi.name.clone();
    }
    match c.station.callsign.trim() {
        "" => "web888d".into(),
        call => format!("web888d {call}"),
    }
}

/// The centre of a Maidenhead locator (4 or 6 characters), lat/lon.
pub fn locator_pos(loc: &str) -> Option<(f64, f64)> {
    let b = loc.trim().to_ascii_uppercase().into_bytes();
    if !(b.len() == 4 || b.len() == 6) {
        return None;
    }
    let f = |c: u8, base: u8, n: u8| (c >= base && c < base + n).then(|| (c - base) as f64);
    let mut lon = -180.0 + 20.0 * f(b[0], b'A', 18)? + 2.0 * f(b[2], b'0', 10)?;
    let mut lat = -90.0 + 10.0 * f(b[1], b'A', 18)? + f(b[3], b'0', 10)?;
    if b.len() == 6 {
        lon += f(b[4], b'A', 24)? / 12.0 + 1.0 / 24.0;
        lat += f(b[5], b'A', 24)? / 24.0 + 1.0 / 48.0;
    } else {
        lon += 1.0;
        lat += 0.5;
    }
    Some((lat, lon))
}

/// `rx_gps` as Kiwi has it: "(lat, lon)".
pub(crate) fn rx_gps(c: &web888_config::Config) -> String {
    let (lat, lon) = locator_pos(&c.station.locator).unwrap_or((0.0, 0.0));
    format!("({lat:.6}, {lon:.6})")
}

/// The configuration a Kiwi client reads (`load_cfg`): what kiwiclient
/// and listing tools look at.
pub(crate) fn cfg_json(sh: &Shared) -> serde_json::Value {
    let c = sh.live.borrow().clone();
    let g = Geometry::new(&sh.radio);
    serde_json::json!({
        "rx_name": rx_name(&c),
        "rx_location": c.kiwi.location,
        "rx_gps": rx_gps(&c),
        "rx_grid": c.station.locator,
        "rx_antenna": c.station.antenna,
        "rx_asl": 0,
        "freq_offset": g.rf_lo / 1e3,
        "max_freq": if g.bandwidth > 30.1e6 { 1 } else { 0 },
        "S_meter_cal": 0,
        "waterfall_cal": 0,
        "server_url": "",
    })
}

/// A binary `MSG` frame; false when the client is gone.
pub(crate) async fn send_msg(ws: &mut WebSocket, text: &str) -> bool {
    ws.send(Message::Binary(wire::msg(text).into()))
        .await
        .is_ok()
}

/// A client command as text (Kiwi clients send text frames; some
/// binary). None: not a command (a ping, an empty frame).
pub(crate) fn command(m: &Message) -> Option<String> {
    match m {
        Message::Text(t) => Some(t.to_string()),
        Message::Binary(b) => Some(String::from_utf8_lossy(b).into_owned()),
        _ => None,
    }
    .filter(|s| !s.is_empty())
}

/// What a password gives: the role, or the `badp` code to refuse with.
pub(crate) async fn check_password(
    sh: &Shared,
    ip: IpAddr,
    admin: bool,
    password: String,
) -> Result<Role, u8> {
    let a = sh.live.borrow().auth.clone();
    if !admin && a.listener_hash.is_none() {
        return Ok(Role::Listener);
    }
    if admin && a.admin_hash.is_none() {
        return Err(BADP_NO_ADMIN_PWD_SET);
    }
    if password.is_empty() || password.len() > 1024 {
        return Err(BADP_TRY_AGAIN);
    }
    let want = if admin { Role::Admin } else { Role::Listener };
    // one check at a time, failures limited per address (as for logins)
    let _turn = sh.admin.logins.checking.lock().await;
    if sh.admin.logins.blocked(ip).is_some() {
        return Err(BADP_TRY_AGAIN);
    }
    let got = tokio::task::spawn_blocking(move || crate::auth::check(&a, &password, want))
        .await
        .ok()
        .flatten();
    match got {
        Some((role, _)) if role >= want => Ok(role),
        _ => {
            sh.admin.logins.failed(ip);
            Err(BADP_TRY_AGAIN)
        }
    }
}

/// Wait for `SET auth` (keepalives and options may come first; anything
/// else before it ends the stream, as upstream) and check it: Some(role)
/// or Some(badp code); None when the client went away or took too long.
pub(crate) async fn wait_auth(
    ws: &mut WebSocket,
    sh: &Shared,
    ip: IpAddr,
) -> Option<Result<Role, u8>> {
    let deadline = tokio::time::Instant::now() + KEEPALIVE_NO_AUTH;
    loop {
        let m = tokio::time::timeout_at(deadline, ws.recv()).await;
        let Ok(Some(Ok(m))) = m else {
            if m.is_err() {
                let _ = send_msg(ws, "password_timeout").await;
            }
            return None;
        };
        if matches!(m, Message::Close(_)) {
            return None;
        }
        let Some(text) = command(&m) else { continue };
        match wire::Cmd::parse(&text) {
            wire::Cmd::Auth { admin, password } => {
                return Some(check_password(sh, ip, admin, password).await);
            }
            wire::Cmd::Keepalive => {}
            _ if text.starts_with("SET options") => {}
            _ => {
                let _ = ws
                    .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                        code: 1008,
                        reason: "no auth yet".into(),
                    })))
                    .await;
                return None;
            }
        }
    }
}

/// The answer to `SET auth`, as upstream sends it.
pub(crate) async fn answer_auth(ws: &mut WebSocket, sh: &Shared, badp: u8) -> bool {
    let n = sh.channels.channels();
    for t in [
        format!("rx_chans={n}"),
        format!("chan_no_pwd={n}"),
        format!("chan_no_pwd_true={n}"),
        "max_camp=0".into(),
        format!("badp={badp}"),
    ] {
        if !send_msg(ws, &t).await {
            return false;
        }
    }
    true
}

/// What every accepted stream gets: the version and the configuration.
pub(crate) async fn send_config(ws: &mut WebSocket, sh: &Shared) -> bool {
    let cfg = wire::url_encode(&cfg_json(sh).to_string());
    let empty = wire::url_encode("{}");
    for t in [
        format!("version_maj={VERSION_MAJ} version_min={VERSION_MIN} platform=0 ext_clk=0"),
        format!("load_cfg={cfg}"),
        format!("load_dxcfg={empty}"),
        format!("load_dxcomm_cfg={empty}"),
        "cfg_loaded".into(),
    ] {
        if !send_msg(ws, &t).await {
            return false;
        }
    }
    true
}

/// The setup messages both streams start with: offset and band.
pub(crate) fn band_msgs(sh: &Shared) -> [String; 2] {
    let g = Geometry::new(&sh.radio);
    [
        format!("freq_offset={:.3}", g.rf_lo / 1e3),
        format!(
            "center_freq={:.0} bandwidth={:.0} adc_clk_nom={:.0}",
            g.bandwidth / 2.0,
            g.bandwidth,
            sh.radio.adc_hz
        ),
    ]
}

/// The audio rate as the ADC clock makes it (PPS-corrected).
pub(crate) fn sample_rate(sh: &Shared) -> f64 {
    let ppb = sh.status.clock_ppb.load(Ordering::Relaxed) as f64;
    web888_dsp::audio::AUDIO_RATE * (1.0 + ppb * 1e-9)
}

/// The routes: the streams (`/<n>/SND`, `/<n>/W/F`, also under `/kiwi/`
/// and `/no_wf/` as Kiwi's UI names them), `/status` and `/VER`.
#[cfg(feature = "kiwi")]
pub fn routes() -> axum::Router<Arc<Shared>> {
    use axum::routing::get;
    axum::Router::new()
        .route("/{ts}/SND", get(snd::upgrade))
        .route("/{ts}/W/F", get(wf::upgrade))
        .route("/kiwi/{ts}/SND", get(snd::upgrade))
        .route("/kiwi/{ts}/W/F", get(wf::upgrade))
        .route("/no_wf/{ts}/SND", get(snd::upgrade))
        .route("/status", get(status))
        .route("/VER", get(version))
}

#[cfg(not(feature = "kiwi"))]
pub fn routes() -> axum::Router<Arc<Shared>> {
    axum::Router::new()
}

/// A stream's timestamp from its URL; None: not a Kiwi stream URL.
#[cfg(feature = "kiwi")]
pub(crate) fn timestamp(s: &str) -> Option<u64> {
    s.parse().ok()
}

#[cfg(feature = "kiwi")]
async fn version(
    axum::extract::State(sh): axum::extract::State<Arc<Shared>>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if !sh.live.borrow().kiwi.enabled {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    axum::Json(serde_json::json!({ "maj": VERSION_MAJ, "min": VERSION_MIN, "ts": ts }))
        .into_response()
}

/// `/status`: `key=value` lines, as Kiwi listings read them.
#[cfg(feature = "kiwi")]
async fn status(
    axum::extract::State(sh): axum::extract::State<Arc<Shared>>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let c = sh.live.borrow().clone();
    if !c.kiwi.enabled {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }
    let g = Geometry::new(&sh.radio);
    let n = sh.channels.channels();
    let users = sh
        .channels
        .leases()
        .iter()
        .filter(|l| l.state.channel().is_some())
        .count()
        .min(n);
    let up = sh.kiwi.started.elapsed().as_secs();
    let body = format!(
        "status=private\n{auth}offline=no\nname={name}\nsdr_hw=web888d {ver}\n\
         op_email=\nbands={lo:.0}-{hi:.0}\nfreq_offset={off:.3}\nusers={users}\n\
         users_max={n}\npreempt=0\navatar_ctime=0\ngps={gps}\ngps_good=0\nfixes=0\n\
         fixes_min=0\nfixes_hour=0\ntdoa_id=\ntdoa_ch=-1\nasl=0\nloc={loc}\n\
         sw_version=web888d_v{ver}\nantenna={ant}\nsnr=0,0\nant_connected=1\n\
         adc_ov={ov}\nclk_ext_gps=0,0\nadc_clk={clk:.6}\nuptime={up}\ndate={date}\n",
        auth = if c.auth.listener_hash.is_some() {
            "auth=password\n"
        } else {
            ""
        },
        name = rx_name(&c),
        ver = env!("CARGO_PKG_VERSION"),
        lo = g.rf_lo,
        hi = g.rf_lo + g.bandwidth,
        off = g.rf_lo / 1e3,
        gps = rx_gps(&c),
        loc = c.kiwi.location,
        ant = c.station.antenna,
        ov = sh.status.overload_blocks.load(Ordering::Relaxed),
        clk = sh.radio.adc_hz / 1e6
            * (1.0 + sh.status.clock_ppb.load(Ordering::Relaxed) as f64 * 1e-9),
        date = httpdate(std::time::SystemTime::now()),
    );
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

/// "Wed Nov 11 10:20:30 2026" (UTC), as `ctime` prints it.
#[cfg(feature = "kiwi")]
fn httpdate(t: std::time::SystemTime) -> String {
    let s = t
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as i64;
    let days = s.div_euclid(86_400);
    let sec = s.rem_euclid(86_400);
    // civil date from days since 1970-01-01 (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "{} {} {:2} {:02}:{:02}:{:02} {}",
        DAYS[days.rem_euclid(7) as usize],
        MONTHS[(m - 1) as usize],
        d,
        sec / 3600,
        sec / 60 % 60,
        sec % 60,
        y
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locators_give_their_centres() {
        let (lat, lon) = locator_pos("FN42").unwrap();
        assert!((lat - 42.5).abs() < 1e-9 && (lon + 71.0).abs() < 1e-9);
        let (lat, lon) = locator_pos("fn42aa").unwrap();
        assert!((lat - (42.0 + 1.0 / 48.0)).abs() < 1e-9);
        assert!((lon - (-72.0 + 1.0 / 24.0)).abs() < 1e-9);
        assert_eq!(locator_pos(""), None);
        assert_eq!(locator_pos("ZZ99"), None);
    }

    #[cfg(feature = "kiwi")]
    #[test]
    fn ctime_dates() {
        let t = std::time::UNIX_EPOCH + Duration::from_secs(1_794_392_430);
        assert_eq!(httpdate(t), "Wed Nov 11 10:20:30 2026");
        assert_eq!(httpdate(std::time::UNIX_EPOCH), "Thu Jan  1 00:00:00 1970");
    }

    #[test]
    fn geometry_from_zero_or_an_offset() {
        let mut r = RadioInfo {
            backend: "sim".into(),
            variant: "hf".into(),
            adc_hz: 122_880_000.0,
            rx_rate: 48_000.0,
            rf_lo: 0.0,
            rf_hi: 61_440_000.0,
            wf_decim_min: 2,
            wf_decim_max: 8192,
        };
        let g = Geometry::new(&r);
        assert_eq!(g.span(0), 61_440_000.0);
        assert_eq!(g.span(14), 3750.0);
        assert!((g.hz_per_start() - 61_440_000.0 / 16_777_216.0).abs() < 1e-9);
        r.rf_lo = 6e6;
        r.rf_hi = 8e6;
        assert_eq!(Geometry::new(&r).bandwidth, 2e6);
    }
}
