// SPDX-License-Identifier: GPL-3.0-or-later
//! The admin page's Clock tab (`GET /api/admin/clock`): where the time
//! comes from, in one answer. chrony's view (`chronyc -c`: its tracking
//! and sources, the receiver's own `LF` among them), the kernel's clock
//! discipline, the receiver's timebase, what [`crate::timesync`] sends
//! to chrony and the time-code stations ([`crate::tcmon`]).

use crate::admin::AdminOnly;
use crate::session::Shared;
use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub async fn clock(_: AdminOnly, State(sh): State<Arc<Shared>>) -> Json<Value> {
    let chrony = tokio::task::spawn_blocking(chrony)
        .await
        .unwrap_or_else(|_| json!({ "available": false }));
    let timebase = sh.waterfalls.tap().map(|t| {
        let i = t.timebase().info();
        json!({
            "source": i.source.name(),
            "uncertainty_s": i.uncertainty,
            "adc_ppm": i.adc_ppm,
        })
    });
    let kernel = crate::sysstat::kernel_discipline().map(|(synced, est, max, freq)| {
        json!({
            "synced": synced,
            "esterror_s": est,
            "maxerror_s": max,
            "freq_ppm": freq,
        })
    });
    let tc = crate::tcmon::snapshot().map(|s| {
        let rows: Vec<Value> = s
            .reports
            .iter()
            .zip(s.counts.iter().chain(std::iter::repeat(&[0u64; 3])))
            .map(|(r, c)| {
                let mut v = serde_json::to_value(r).unwrap_or(Value::Null);
                v["minutes"] = json!({ "ok": c[0], "unconfirmed": c[1], "failed": c[2] });
                v
            })
            .collect();
        json!({ "stations": rows })
    });
    Json(json!({
        "now": crate::sysstat::clock_s(libc::CLOCK_REALTIME),
        "chrony": chrony,
        "kernel": kernel,
        "timebase": timebase,
        "timesync": crate::timesync::snapshot(),
        "timecode": tc,
        "ntp_server": ntp_server(&sh),
    }))
}

/// The clock served to the LAN: the setting, whether chrony took it and
/// the NTP requests chrony has had (`chronyc serverstats`).
fn ntp_server(sh: &Shared) -> Value {
    let enabled = sh.live.borrow().timesync.serve_lan;
    let stats = chronyc(&["serverstats"]).and_then(|v| v.into_iter().next());
    let n = |i: usize| stats.as_ref().and_then(|v| v.get(i)?.parse::<u64>().ok());
    json!({
        "enabled": enabled,
        "status": crate::ntpserve::snapshot(),
        "requests": n(0),
        "dropped": n(1),
    })
}

/// `chronyc -c ARGS`' lines, its fields split; none when chronyc isn't
/// there or doesn't answer within 3 s (chronyd not running).
fn chronyc(args: &[&str]) -> Option<Vec<Vec<String>>> {
    let mut child = Command::new("chronyc")
        .arg("-c")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let t0 = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) if st.success() => break,
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if t0.elapsed() > Duration::from_secs(3) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    let out = std::io::read_to_string(child.stdout.take()?).ok()?;
    Some(
        out.lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.split(',').map(String::from).collect())
            .collect(),
    )
}

fn chrony() -> Value {
    let Some(tr) = chronyc(&["-n", "tracking"]) else {
        return json!({ "available": false });
    };
    let f = |v: &[String], i: usize| v.get(i).and_then(|x| x.parse::<f64>().ok());
    let tracking = tr.first().map(|v| {
        json!({
            "ref_id": v.first(),
            "ref_name": v.get(1),
            "stratum": f(v, 2),
            "ref_time": f(v, 3),
            // + : the system clock is slow (chrony is advancing it)
            "slow_s": f(v, 4),
            "last_offset_s": f(v, 5),
            "rms_offset_s": f(v, 6),
            // + : fast (chronyc says "slow" for -)
            "freq_ppm": f(v, 7),
            "residual_freq_ppm": f(v, 8),
            "skew_ppm": f(v, 9),
            "root_delay_s": f(v, 10),
            "root_dispersion_s": f(v, 11),
            "update_interval_s": f(v, 12),
            "leap": v.get(13),
        })
    });
    let sources: Vec<Value> = chronyc(&["-n", "sources"])
        .unwrap_or_default()
        .iter()
        .map(|v| {
            json!({
                // ^ server, = peer, # reference clock
                "mode": v.first(),
                // * selected, + combined, - not combined, ? unusable,
                // x a falseticker, ~ too variable
                "state": v.get(1),
                "name": v.get(2),
                "stratum": f(v, 3),
                "poll": f(v, 4),
                "reach": v.get(5).and_then(|r| u32::from_str_radix(r, 8).ok()),
                "last_rx_s": f(v, 6),
                // + : the system clock is ahead of the source
                "offset_s": f(v, 7),
                "measured_offset_s": f(v, 8),
                "error_s": f(v, 9),
            })
        })
        .collect();
    json!({ "available": true, "tracking": tracking, "sources": sources })
}
