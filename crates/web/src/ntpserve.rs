// SPDX-License-Identifier: GPL-3.0-or-later
//! The system clock served to the local network over NTP (`[timesync]
//! serve_lan`): chrony answers clients from private and link-local
//! addresses (`chronyc allow`), or no one (`chronyc deny all`, chrony's
//! own default). Applied when the setting changes and again every 10
//! minutes while on (a restarted chronyd forgets it). For a station
//! without good Internet (a DXpedition): the receiver's clock, kept by
//! the time-code stations, as the shack's reference.

use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;
use web888_config::Config;

/// The clients served: IPv4's private ranges and link-local, IPv6's
/// unique-local and link-local. Not the Internet: a global address
/// (IPv6 on most networks) is refused; and a router forwarding UDP 123
/// to the receiver would hand it private source addresses' traffic only.
pub const LAN: [&str; 6] = [
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "169.254.0.0/16",
    "fc00::/7",
    "fe80::/10",
];

/// How often the setting is applied again while on (s).
const REAPPLY_S: u64 = 600;

#[derive(Clone, Debug, serde::Serialize)]
pub struct Status {
    /// The setting.
    pub enabled: bool,
    /// chrony took it (the last time it was applied).
    pub applied: bool,
    /// Why not, if it didn't.
    pub error: Option<String>,
    /// When it was last applied (Unix s).
    pub at: f64,
    /// The networks served (when on).
    pub networks: Vec<&'static str>,
}

static STATE: Mutex<Option<Status>> = Mutex::new(None);

/// The last application's result (none before the first).
pub fn snapshot() -> Option<Status> {
    STATE.lock().ok()?.clone()
}

/// Apply `[timesync] serve_lan` now, when it changes and every
/// [`REAPPLY_S`] while on.
pub async fn run(mut live: watch::Receiver<Arc<Config>>) {
    let mut last: Option<bool> = None;
    loop {
        let on = live.borrow_and_update().timesync.serve_lan;
        if last != Some(on) || on {
            let r = tokio::task::spawn_blocking(move || apply(on))
                .await
                .unwrap_or_else(|e| Err(e.to_string()));
            // once per failure; never at an off start (chrony's default:
            // and on a desktop chronyc may not be ours to use)
            if let Err(e) = &r {
                let prev = snapshot();
                let news = prev.as_ref().is_none_or(|p| p.applied || p.enabled != on);
                if news && (on || last == Some(true)) {
                    eprintln!(
                        "ntpserve: serving the LAN {}: {e}",
                        if on { "on" } else { "off" }
                    );
                }
            }
            if let Ok(mut s) = STATE.lock() {
                *s = Some(Status {
                    enabled: on,
                    applied: r.is_ok(),
                    error: r.err(),
                    at: crate::sysstat::clock_s(libc::CLOCK_REALTIME),
                    networks: if on { LAN.to_vec() } else { Vec::new() },
                });
            }
            last = Some(on);
        }
        tokio::select! {
            r = live.changed() => if r.is_err() { return },
            _ = tokio::time::sleep(Duration::from_secs(REAPPLY_S)) => {}
        }
    }
}

/// chrony's access rules: the [`LAN`] allowed, or everyone denied.
fn apply(on: bool) -> Result<(), String> {
    if on {
        LAN.iter().try_for_each(|n| chronyc(&["allow", n]))
    } else {
        chronyc(&["deny", "all"])
    }
}

/// One chronyc command over its local socket (root's: web888d's on the
/// unit), within 3 s.
fn chronyc(args: &[&str]) -> Result<(), String> {
    let mut child = Command::new("chronyc")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("chronyc: {e}"))?;
    let t0 = std::time::Instant::now();
    let st = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if t0.elapsed() > Duration::from_secs(3) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("chronyc: no answer (is chronyd running?)".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(format!("chronyc: {e}")),
        }
    };
    let out = child
        .stdout
        .take()
        .and_then(|o| std::io::read_to_string(o).ok())
        .unwrap_or_default();
    let err = child
        .stderr
        .take()
        .and_then(|o| std::io::read_to_string(o).ok())
        .unwrap_or_default();
    if st.success() && out.trim_start().starts_with("200") {
        Ok(())
    } else {
        let msg = format!("{} {}", out.trim(), err.trim());
        Err(format!("chronyc {}: {}", args.join(" "), msg.trim()))
    }
}
