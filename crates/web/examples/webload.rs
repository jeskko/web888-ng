// SPDX-License-Identifier: GPL-3.0-or-later
//! Load tool (`doc/m3-multi-user.md` §8): N WebSocket sessions against a
//! web888d, each with its own dial and waterfall view; prints per session
//! the audio rate, gaps, waterfall lines per second and how many of them
//! had new samples (the achieved rate of a time-shared view).
//!
//!   cargo run --release -p web888-web --example webload -- \
//!       ws://HOST:8073/ws [--sessions 13] [--secs 10] \
//!       [--views mixed|same|distinct] [--base 7000000]
//!
//! Views: `same`: everyone on the default full view (one shared capture);
//! `distinct`: everyone 480 kHz around their own dial (as many captures
//! as sessions: time-shared slots); `mixed`: in turn the full view, 30 kHz
//! around the dial (drawn from the RX channel), 480 kHz around the dial,
//! and a 1.92 MHz view shared by all who have it.
//! The server must allow the sessions from one address (`limits.per_ip`).

use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

const KIND_AUDIO: u8 = 1;
const KIND_WF: u8 = 2;
const FLAG_GAP: u8 = 2;
const FLAG_STALE: u8 = 4;
const WARMUP: Duration = Duration::from_millis(1500);

#[derive(Clone, Copy, PartialEq)]
enum Views {
    Same,
    Distinct,
    Mixed,
}

#[derive(Default)]
struct Stats {
    label: String,
    audio: usize,
    gaps: usize,
    lines: usize,
    fresh: usize,
    drops: (u64, u64),
    error: Option<String>,
}

/// Session k's dial and view request (none: the default full view).
fn plan(k: usize, views: Views, base: f64) -> (f64, Option<(f64, u16)>, &'static str) {
    let dial = base + 25_000.0 * k as f64;
    let mode = match views {
        Views::Same => 0,
        Views::Distinct => 2,
        Views::Mixed => k % 4,
    };
    match mode {
        0 => (dial, None, "full"),
        1 => (dial, Some((dial, 4096)), "30k@dial (channel)"),
        2 => (dial, Some((dial, 256)), "480k@dial"),
        _ => (dial, Some((base + 1e6, 64)), "1.92M shared"),
    }
}

async fn session(url: String, k: usize, views: Views, base: f64, secs: f64) -> Stats {
    let (dial, view, what) = plan(k, views, base);
    let mut st = Stats {
        label: format!("{:>10.3} kHz  {what}", dial / 1e3),
        ..Default::default()
    };
    let mut ws = match tokio_tungstenite::connect_async(&url).await {
        Ok((ws, _)) => ws,
        Err(e) => {
            st.error = Some(e.to_string());
            return st;
        }
    };
    let mut msgs = vec![format!(r#"{{"type":"tune","hz":{dial}}}"#)];
    if let Some((c, d)) = view {
        msgs.push(format!(
            r#"{{"type":"wf","center_hz":{c},"decim":{d},"bins":1024,"fps":20}}"#
        ));
    }
    for m in msgs {
        if let Err(e) = ws.send(Message::Text(m.into())).await {
            st.error = Some(e.to_string());
            return st;
        }
    }
    let t0 = Instant::now();
    let start = t0 + WARMUP;
    let end = start + Duration::from_secs_f64(secs);
    while let Ok(Some(m)) = tokio::time::timeout_at(end, ws.next()).await {
        let m = match m {
            Ok(m) => m,
            Err(e) => {
                st.error = Some(e.to_string());
                break;
            }
        };
        let counting = Instant::now() >= start;
        match m {
            Message::Binary(b) if b.len() > 4 && counting => match b[0] {
                KIND_AUDIO => {
                    st.audio += u16::from_le_bytes([b[2], b[3]]) as usize;
                    st.gaps += (b[1] & FLAG_GAP != 0) as usize;
                }
                KIND_WF => {
                    st.lines += 1;
                    st.fresh += (b[1] & FLAG_STALE == 0) as usize;
                }
                _ => {}
            },
            Message::Text(t) => {
                if t.contains(r#""type":"busy""#) || t.contains(r#""type":"error""#) {
                    st.error = Some(t.to_string());
                    break;
                }
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t)
                    && v["type"] == "status"
                {
                    st.drops = (
                        v["audio_drops"].as_u64().unwrap_or(0),
                        v["wf_drops"].as_u64().unwrap_or(0),
                    );
                }
            }
            _ => {}
        }
    }
    let _ = ws.close(None).await;
    st
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let mut url = None;
    let (mut n, mut secs, mut views, mut base) = (13usize, 10.0f64, Views::Mixed, 7e6f64);
    while let Some(a) = args.next() {
        let mut val = || args.next().expect("option value");
        match a.as_str() {
            "--sessions" => n = val().parse().expect("--sessions N"),
            "--secs" => secs = val().parse().expect("--secs S"),
            "--base" => base = val().parse().expect("--base HZ"),
            "--views" => {
                views = match val().as_str() {
                    "same" => Views::Same,
                    "distinct" => Views::Distinct,
                    "mixed" => Views::Mixed,
                    v => panic!("--views {v}: same, distinct or mixed"),
                }
            }
            _ if url.is_none() => url = Some(a),
            _ => panic!("unexpected argument {a}"),
        }
    }
    let url = url.expect("usage: webload ws://HOST:PORT/ws [options]");
    let tasks: Vec<_> = (0..n)
        .map(|k| tokio::spawn(session(url.clone(), k, views, base, secs)))
        .collect();
    println!(
        "{:>3}  {:<34} {:>9} {:>5} {:>7} {:>9} {:>11}",
        "#", "dial / view", "audio S/s", "gaps", "wf fps", "fresh fps", "drops a/wf"
    );
    let mut fresh = Vec::new();
    for (k, t) in tasks.into_iter().enumerate() {
        let s = t.await.expect("session task");
        if let Some(e) = &s.error {
            println!("{k:>3}  {:<34} error: {e}", s.label);
            continue;
        }
        let f = s.fresh as f64 / secs;
        fresh.push(f);
        println!(
            "{k:>3}  {:<34} {:>9.0} {:>5} {:>7.1} {:>9.1} {:>5}/{:<5}",
            s.label,
            s.audio as f64 / secs,
            s.gaps,
            s.lines as f64 / secs,
            f,
            s.drops.0,
            s.drops.1
        );
    }
    if !fresh.is_empty() {
        let min = fresh.iter().copied().fold(f64::MAX, f64::min);
        let max = fresh.iter().copied().fold(0.0, f64::max);
        println!("fresh lines/s: min {min:.1}, max {max:.1}");
    }
}
