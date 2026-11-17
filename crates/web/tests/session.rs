// SPDX-License-Identifier: GPL-3.0-or-later
//! A web session end to end: simulated radio -> hw loop -> server on a
//! loopback port -> WebSocket client.

use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;
use web888_hw::{HwLoop, HwStatus};
use web888_radio::*;
use web888_sim::*;
use web888_web::proto::{self, RadioInfo, ServerMsg};
use web888_web::session::Shared;

const TONE: f64 = 7_075_500.0;

async fn start() -> (std::net::SocketAddr, Arc<AtomicBool>) {
    let (addr, stop, _) = start_with(web888_config::Config::default()).await;
    (addr, stop)
}

async fn start_with_channels() -> (
    std::net::SocketAddr,
    Arc<AtomicBool>,
    Arc<web888_core::leases::Channels>,
) {
    start_with(web888_config::Config::default()).await
}

async fn start_with(
    config: web888_config::Config,
) -> (
    std::net::SocketAddr,
    Arc<AtomicBool>,
    Arc<web888_core::leases::Channels>,
) {
    start_skim(config, None).await
}

async fn start_skim(
    config: web888_config::Config,
    skim: Option<web888_web::session::Skim>,
) -> (
    std::net::SocketAddr,
    Arc<AtomicBool>,
    Arc<web888_core::leases::Channels>,
) {
    let scene = Scene::new().tone(TONE, -40.0).adc_noise();
    let mut radio = SimRadio::new(SimConfig::hf(scene, Pacing::RealTime));
    let caps = radio.caps();
    let r2 = r2_for_rate(caps.adc_hz, 48_000.0);
    radio.set_rx_rate(r2).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let status = Arc::new(HwStatus::default());
    let hw = HwLoop {
        radio: Box::new(radio),
        ctl: rx,
        stop: stop.clone(),
        status: status.clone(),
        verbose: false,
    };
    std::thread::spawn(move || hw.run());
    let channels = web888_core::leases::Channels::new(
        caps.rx_channels,
        web888_core::leases::Policy::default(),
        tx.clone(),
    );
    let pool = Arc::new(web888_core::pool::DspPool::new(2, &tx, false).unwrap());
    let shared = Arc::new(Shared {
        waterfalls: web888_web::wfsvc::Waterfalls::new(
            caps.wf_channels,
            caps.adc_hz,
            caps.wf_block,
            tx.clone(),
            pool.clone(),
        ),
        status,
        radio: RadioInfo {
            backend: "sim".into(),
            variant: "hf".into(),
            adc_hz: caps.adc_hz,
            rx_rate: rx_rate(caps.adc_hz, r2),
            rf_lo: *caps.rf_range.start(),
            rf_hi: *caps.rf_range.end(),
            wf_decim_min: caps.wf_decim_min,
            wf_decim_max: caps.wf_decim_max,
            freq_offset_hz: 0.0,
        },
        channels: channels.clone(),
        pool,
        rx_r2: r2,
        live: tokio::sync::watch::channel(Arc::new(config)).1,
        ips: Default::default(),
        sessions: Default::default(),
        next_session: Default::default(),
        users: Default::default(),
        admin: Default::default(),
        skim,
        limit_unit: std::time::Duration::from_millis(100),
        kiwi: Default::default(),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = web888_web::router(shared, "/nonexistent".into());
    tokio::spawn(web888_web::serve(listener, app, None));
    (addr, stop, channels)
}

fn json(m: &Message) -> Option<ServerMsg> {
    match m {
        Message::Text(t) => Some(serde_json::from_str(t).unwrap()),
        _ => None,
    }
}

/// Frequency of the strongest component of real audio (Hann + FFT).
fn dominant_hz(x: &[f32], fs: f64) -> f64 {
    let n = x.len().next_power_of_two() / 2;
    let mut buf: Vec<num_complex::Complex32> = x[..n]
        .iter()
        .enumerate()
        .map(|(k, &v)| {
            let w = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * k as f32 / n as f32).cos();
            num_complex::Complex32::new(v * w, 0.0)
        })
        .collect();
    rustfft::FftPlanner::new()
        .plan_fft_forward(n)
        .process(&mut buf);
    let k = (1..n / 2)
        .max_by(|&a, &b| buf[a].norm().total_cmp(&buf[b].norm()))
        .unwrap();
    k as f64 * fs / n as f64
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_streams_audio_and_waterfall() {
    let (addr, stop) = start().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();

    let hello = json(&ws.next().await.unwrap().unwrap()).unwrap();
    let ServerMsg::Hello {
        proto: 2,
        audio_rate,
        radio,
        ..
    } = hello
    else {
        panic!("{hello:?}")
    };
    assert_eq!(audio_rate, 12_000.0);
    assert_eq!(radio.rf_hi, ADC_HZ_HF / 2.0);
    let ServerMsg::State(st) = json(&ws.next().await.unwrap().unwrap()).unwrap() else {
        panic!()
    };
    assert_eq!(st.hz, 7_066_000.0);
    assert_eq!(st.mode, proto::Mode::Lsb, "LSB below 10 MHz");
    assert_eq!(
        (st.wf.decim, st.wf.center_hz),
        (128, 7_066_000.0),
        "960 kHz around the dial"
    );
    // the checks below listen on 7.074 MHz, USB
    ws.send(Message::Text(r#"{"type":"mode","mode":"usb"}"#.into()))
        .await
        .unwrap();
    ws.send(Message::Text(r#"{"type":"tune","hz":7074000}"#.into()))
        .await
        .unwrap();

    // a second user gets in too
    let (mut ws2, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let m = json(&ws2.next().await.unwrap().unwrap()).unwrap();
    assert!(matches!(m, ServerMsg::Hello { .. }), "{m:?}");

    // zoom: 480 kHz around 7.1 MHz
    let req = r#"{"type":"wf","center_hz":7100000,"decim":256,"bins":1024,"fps":20}"#;
    ws.send(Message::Text(req.into())).await.unwrap();

    let mut audio: Vec<f32> = Vec::new();
    let mut rssi = Vec::new();
    let mut lines = 0;
    let mut new_gen = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while (audio.len() < 18_000 || lines < 5) && tokio::time::Instant::now() < deadline {
        let m = ws.next().await.unwrap().unwrap();
        let Message::Binary(b) = m else { continue };
        match b[0] {
            proto::KIND_AUDIO => {
                let n = u16::from_le_bytes([b[2], b[3]]) as usize;
                assert_eq!(b.len(), proto::AUDIO_HEADER + 2 * n);
                rssi.push(f32::from_le_bytes(b[12..16].try_into().unwrap()));
                audio.extend(
                    b[16..]
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|c| i16::from_le_bytes(*c) as f32 / 32768.0),
                );
            }
            proto::KIND_WF => {
                new_gen |= b[1] & proto::FLAG_NEW_GEN != 0;
                if !new_gen {
                    continue; // still the full view
                }
                let bins = u16::from_le_bytes([b[2], b[3]]) as usize;
                let f0 = f64::from_le_bytes(b[8..16].try_into().unwrap());
                let df = f64::from_le_bytes(b[16..24].try_into().unwrap());
                let lv = &b[proto::WF_HEADER..];
                assert_eq!((bins, lv.len()), (1024, 1024));
                assert!((df - ADC_HZ_HF / 256.0 / 1024.0).abs() < 1e-6);
                let k = (0..bins).max_by_key(|&k| lv[k]).unwrap();
                let f = f0 + k as f64 * df;
                assert!((f - TONE).abs() <= df, "peak at {f}");
                lines += 1;
            }
            k => panic!("frame kind {k}"),
        }
    }
    assert!(lines >= 5, "{lines} waterfall lines");
    assert!(audio.len() >= 18_000, "{} audio samples", audio.len());
    // USB on 7.074 MHz: the tone at +1500 Hz (after the AGC settled)
    let f = dominant_hz(&audio[6000..], 12_000.0);
    assert!((f - 1500.0).abs() < 10.0, "audio tone {f} Hz");
    let p = rssi[rssi.len() - 5];
    assert!((p + 40.0).abs() < 1.0, "rssi {p}");

    stop.store(true, std::sync::atomic::Ordering::Relaxed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sessions_per_address_are_limited() {
    let mut config = web888_config::Config::default();
    config.limits.per_ip = 2;
    let (addr, stop, _) = start_with(config).await;
    let url = format!("ws://{addr}/ws");
    let mut first = Vec::new();
    for _ in 0..2 {
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let m = json(&ws.next().await.unwrap().unwrap()).unwrap();
        assert!(matches!(m, ServerMsg::Hello { .. }), "{m:?}");
        first.push(ws);
    }
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let m = json(&ws.next().await.unwrap().unwrap()).unwrap();
    assert!(matches!(m, ServerMsg::Busy { .. }), "{m:?}");
    // one leaves: the next one gets in
    let mut ws = first.pop().unwrap();
    ws.close(None).await.unwrap();
    drop(ws);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let m = json(&ws.next().await.unwrap().unwrap()).unwrap();
    assert!(matches!(m, ServerMsg::Hello { .. }), "{m:?}");
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trusted_addresses_are_not_limited() {
    let mut config = web888_config::Config::default();
    config.limits.per_ip = 1;
    config.limits.trusted = vec!["127.0.0.0/8".parse().unwrap()];
    let (addr, stop, _) = start_with(config).await;
    let url = format!("ws://{addr}/ws");
    let mut open = Vec::new();
    for _ in 0..3 {
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let m = json(&ws.next().await.unwrap().unwrap()).unwrap();
        assert!(matches!(m, ServerMsg::Hello { .. }), "{m:?}");
        open.push(ws);
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
}

#[tokio::test]
async fn status_and_missing_ui() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (addr, stop) = start().await;
    for (path, want) in [
        ("/api/status", "\"backend\":\"sim\""),
        ("/", "UI files are not installed"),
    ] {
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(
            format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();
        let mut r = String::new();
        s.read_to_string(&mut r).await.unwrap();
        assert!(r.starts_with("HTTP/1.1 200"), "{r}");
        assert!(r.contains(want), "{path}: {r}");
        assert!(
            r.to_lowercase().contains("cache-control: no-cache"),
            "{path}: {r}"
        );
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Audio samples received within `t` (other messages skipped).
async fn audio_within<S>(ws: &mut S, t: Duration) -> usize
where
    S: futures_util::Stream<
            Item = std::result::Result<Message, tokio_tungstenite::tungstenite::Error>,
        > + Unpin,
{
    let deadline = tokio::time::Instant::now() + t;
    let mut n = 0;
    while let Ok(Some(Ok(m))) = tokio::time::timeout_at(deadline, ws.next()).await {
        if let Message::Binary(b) = m
            && b[0] == proto::KIND_AUDIO
        {
            n += u16::from_le_bytes([b[2], b[3]]) as usize;
        }
    }
    n
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_waits_for_a_channel() {
    use web888_core::leases::{Class, LeaseState};
    let (addr, stop, channels) = start_with_channels().await;
    // HPSDR-like clients hold every channel
    let held: Vec<_> = (0..12)
        .map(|k| channels.acquire(Class::Protocol, "test", 1e6 * k as f64))
        .collect();
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    assert_eq!(audio_within(&mut ws, Duration::from_millis(700)).await, 0);
    let web = channels
        .leases()
        .into_iter()
        .find(|l| l.class == Class::Web)
        .unwrap();
    assert_eq!(web.state, LeaseState::Waiting { position: 1 });
    assert!(web.label.starts_with("web 127.0.0.1"), "{}", web.label);
    assert_eq!(web.hz, 7_066_000.0);

    // one frees: the session gets that channel, tuned to its dial
    drop(held);
    assert!(audio_within(&mut ws, Duration::from_millis(700)).await > 3000);
    let web = channels.leases();
    assert_eq!(web.len(), 1);
    assert_eq!(web[0].state, LeaseState::Granted { ch: 0 });
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn narrow_view_comes_from_the_channel() {
    let (addr, stop) = start().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    // 30 kHz around the dial (7.074 MHz, USB): inside the RX channel
    ws.send(Message::Text(r#"{"type":"mode","mode":"usb"}"#.into()))
        .await
        .unwrap();
    ws.send(Message::Text(r#"{"type":"tune","hz":7074000}"#.into()))
        .await
        .unwrap();
    let req = r#"{"type":"wf","center_hz":7074000,"decim":4096,"bins":1024,"fps":20}"#;
    ws.send(Message::Text(req.into())).await.unwrap();
    let mut lines = 0;
    let mut new_gen = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while lines < 5 && tokio::time::Instant::now() < deadline {
        let m = ws.next().await.unwrap().unwrap();
        let Message::Binary(b) = m else { continue };
        if b[0] != proto::KIND_WF {
            continue;
        }
        new_gen |= b[1] & proto::FLAG_NEW_GEN != 0;
        let bins = u16::from_le_bytes([b[2], b[3]]) as usize;
        let f0 = f64::from_le_bytes(b[8..16].try_into().unwrap());
        let df = f64::from_le_bytes(b[16..24].try_into().unwrap());
        if !new_gen || df > 1000.0 {
            continue; // still the full view
        }
        // the channel's grid: 48 kHz / 2048 bins, cropped to the 30 kHz
        assert!((df - 48_000.0 / 2048.0).abs() < 1e-6, "df {df}");
        let span = bins as f64 * df;
        assert!((span - 30_000.0).abs() <= 2.0 * df, "span {span}");
        assert!((f0 - (7_074_000.0 - 15_000.0)).abs() <= df, "f0 {f0}");
        let lv = &b[proto::WF_HEADER..];
        let k = (0..bins).max_by_key(|&k| lv[k]).unwrap();
        let f = f0 + k as f64 * df;
        assert!((f - TONE).abs() <= df, "peak at {f}");
        // -40 dBFS tone: about 100 above the floor of the u8 scale
        let level = -160.0 + lv[k] as f64 * 160.0 / 255.0;
        assert!((level + 40.0).abs() < 3.0, "tone at {level} dBFS");
        lines += 1;
    }
    assert!(lines >= 5, "{lines} lines");
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// What a session saw within `dur`, tuning every 100 ms if `active`:
/// (warnings, the close code if it was closed).
async fn watch(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    dur: Duration,
    active: bool,
) -> (Vec<String>, Option<u16>) {
    let end = tokio::time::Instant::now() + dur;
    let mut next_tune = tokio::time::Instant::now();
    let mut errors = Vec::new();
    loop {
        let now = tokio::time::Instant::now();
        if now >= end {
            return (errors, None);
        }
        if active && now >= next_tune {
            next_tune = now + Duration::from_millis(100);
            let req = r#"{"type":"tune","hz":7074000}"#;
            if ws.send(Message::Text(req.into())).await.is_err() {
                return (errors, None);
            }
        }
        let wait = end.min(if active { next_tune } else { end }) - now;
        match tokio::time::timeout(wait, ws.next()).await {
            Err(_) => {}
            Ok(None) | Ok(Some(Err(_))) => return (errors, None),
            Ok(Some(Ok(Message::Close(f)))) => return (errors, f.map(|f| u16::from(f.code))),
            Ok(Some(Ok(m))) => {
                if let Some(ServerMsg::Error { msg }) = json(&m) {
                    errors.push(msg);
                }
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_and_time_limits_end_sessions() {
    // a "minute" is 100 ms in the tests
    let mut c = web888_config::Config::default();
    c.limits.idle_minutes = 3;
    let (addr, stop, _) = start_with(c).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    // tuning keeps it alive past the idle limit
    let (warn, closed) = watch(&mut ws, Duration::from_millis(700), true).await;
    assert_eq!(closed, None, "{warn:?}");
    // then quiet: a warning, then the end
    let (warn, closed) = watch(&mut ws, Duration::from_millis(1500), false).await;
    assert_eq!(closed, Some(web888_web::session::CLOSE_IDLE));
    assert!(warn.iter().any(|w| w.contains("in a minute")), "{warn:?}");
    assert!(
        warn.iter().any(|w| w.contains("no activity for 3 min")),
        "{warn:?}"
    );
    stop.store(true, std::sync::atomic::Ordering::Relaxed);

    let mut c = web888_config::Config::default();
    c.limits.session_minutes = 5;
    let (addr, stop, _) = start_with(c).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    // activity doesn't extend the time limit
    let (warn, closed) = watch(&mut ws, Duration::from_millis(1500), true).await;
    assert_eq!(
        closed,
        Some(web888_web::session::CLOSE_TIME_LIMIT),
        "{warn:?}"
    );
    assert!(
        warn.iter().any(|w| w.contains("time limit (5 min)")),
        "{warn:?}"
    );
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
}

fn spot(time: f64, call: &str, dial: f64) -> web888_skim::Spot {
    web888_skim::Spot {
        time,
        mode: "FT8",
        dial_hz: dial,
        freq_hz: dial + 1234.4,
        snr_db: -12.3,
        dt_s: 0.24,
        message: format!("CQ {call} FN42"),
        sender: Some(call.into()),
        grid: Some("FN42".into()),
        drift: None,
        dbm: None,
        live: false,
        uncertain: false,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spots_come_recent_first_then_new() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let bus = web888_skim::SpotBus::new(100);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    bus.publish(spot(now - 7200.0, "K1OLD", 7_074_000.0));
    bus.publish(spot(now - 60.0, "K1ABC", 7_074_000.0));
    bus.publish(spot(now - 45.0, "K2ABC", 14_074_000.0));
    let mut config = web888_config::Config::default();
    config.skimmer.push(web888_config::Skimmer {
        dial: 7_074_000.0,
        ..Default::default()
    });
    let skim = web888_web::session::Skim {
        bus: bus.clone(),
        skimmers: None,
        pskreporter: None,
        wsprnet: None,
        gpsd: None,
        live: false,
    };
    let (addr, stop, _) = start_skim(config, Some(skim)).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let hello = json(&ws.next().await.unwrap().unwrap()).unwrap();
    assert!(
        matches!(hello, ServerMsg::Hello { spots: true, .. }),
        "{hello:?}"
    );
    ws.send(Message::text(r#"{"type":"spots","on":true}"#))
        .await
        .unwrap();
    let mut batches = vec![];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while batches.len() < 2 && tokio::time::Instant::now() < deadline {
        let m = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let Some(ServerMsg::Spots { spots }) = json(&m) {
            if batches.is_empty() {
                bus.publish(spot(now, "K3NEW", 7_074_000.0));
            }
            batches.push(spots);
        }
    }
    let calls = |v: &Vec<proto::SpotMsg>| {
        v.iter()
            .map(|s| s.call.clone().unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(calls(&batches[0]), ["K1ABC", "K2ABC"], "the last 30 min");
    assert_eq!(calls(&batches[1]), ["K3NEW"]);
    let s = &batches[0][0];
    assert_eq!(
        (s.band.as_str(), s.hz, s.snr, s.dt),
        ("40m", 7_075_234.0, -12.0, 0.2)
    );

    let mut c = tokio::net::TcpStream::connect(addr).await.unwrap();
    c.write_all(b"GET /api/spots?band=20m HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut r = String::new();
    c.read_to_string(&mut r).await.unwrap();
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    assert!(r.contains("K2ABC") && !r.contains("K1ABC"), "{r}");
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// The next waterfall line of a new view: (f0, df, levels).
async fn next_view_line<S, E>(ws: &mut S) -> (f64, f64, Vec<u8>)
where
    S: futures_util::Stream<Item = std::result::Result<Message, E>> + Unpin,
    E: std::fmt::Debug,
{
    let mut new_gen = false;
    let mut skip = 2; // let the view's first lines settle
    loop {
        let Message::Binary(b) = ws.next().await.unwrap().unwrap() else {
            continue;
        };
        if b[0] != proto::KIND_WF {
            continue;
        }
        new_gen |= b[1] & proto::FLAG_NEW_GEN != 0;
        if !new_gen {
            continue;
        }
        if skip > 0 {
            skip -= 1;
            continue;
        }
        let f0 = f64::from_le_bytes(b[8..16].try_into().unwrap());
        let df = f64::from_le_bytes(b[16..24].try_into().unwrap());
        return (f0, df, b[proto::WF_HEADER..].to_vec());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn waterfalls_show_no_images_or_edge_aliases() {
    let (addr, stop) = start().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let level = |f0: f64, df: f64, lv: &[u8], f: f64| {
        let k = ((f - f0) / df).round() as usize;
        lv[k.saturating_sub(2)..(k + 3).min(lv.len())]
            .iter()
            .copied()
            .max()
            .unwrap() as f32
            * (160.0 / 255.0) // dB per level step
    };
    let fs = ADC_HZ_HF;
    // a 35 dB margin: the old decimated full view let the mirror through
    // about 15 dB down
    let margin = 35.0;

    // the full view: 0..fs/2, the tone and no mirror at fs/2 - tone
    let req = r#"{"type":"wf","center_hz":30720000,"decim":2,"bins":1024,"fps":20}"#;
    ws.send(Message::Text(req.into())).await.unwrap();
    let (f0, df, lv) = next_view_line(&mut ws).await;
    assert!(f0.abs() < df, "starts at 0 Hz: {f0}");
    let tone = level(f0, df, &lv, TONE);
    let mirror = level(f0, df, &lv, fs / 2.0 - TONE);
    assert!(tone - mirror > margin, "tone {tone} dB, mirror {mirror} dB");

    // a 30.72 MHz view whose left edge is 1 MHz above the tone: with an
    // output band no wider than the view, the tone aliased in near the
    // right edge
    let c = TONE + fs / 8.0 + 1e6;
    let req = format!(r#"{{"type":"wf","center_hz":{c},"decim":4,"bins":1024,"fps":20}}"#);
    ws.send(Message::Text(req.into())).await.unwrap();
    let (f0, df, lv) = next_view_line(&mut ws).await;
    let alias = level(f0, df, &lv, c + fs / 8.0 - 1e6);
    let floor = level(f0, df, &lv, c);
    assert!(
        alias - floor < 6.0,
        "alias {alias} dB over a floor of {floor} dB"
    );
    assert!(tone - alias > margin, "tone {tone} dB, alias {alias} dB");

    stop.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// The codec changes at a frame boundary: PCM16, then mu-law, then Opus,
/// each decoded to the simulated tone (USB on 7.074 MHz: +1500 Hz).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codecs_switch_mid_stream() {
    use web888_web::codec::mulaw_decode;
    let (addr, stop) = start().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let ServerMsg::Hello { codecs, .. } = json(&ws.next().await.unwrap().unwrap()).unwrap() else {
        panic!()
    };
    assert_eq!(codecs, ["pcm16", "mulaw", "opus"]);
    for m in [
        r#"{"type":"mode","mode":"usb"}"#,
        r#"{"type":"tune","hz":7074000}"#,
    ] {
        ws.send(Message::Text(m.into())).await.unwrap();
    }
    let mut dec =
        opus1::OpusDecoder::new(opus1::SampleRate::Hz12000, opus1::Channels::Mono).unwrap();
    for (codec, flag) in [
        ("pcm16", 0),
        ("mulaw", proto::FLAG_MULAW),
        ("opus", proto::FLAG_OPUS),
    ] {
        let m = format!(r#"{{"type":"audio","codec":"{codec}"}}"#);
        ws.send(Message::Text(m.into())).await.unwrap();
        let mut audio: Vec<f32> = Vec::new();
        let mut state_codec = None;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while audio.len() < 12_000 && tokio::time::Instant::now() < deadline {
            let m = ws.next().await.unwrap().unwrap();
            if let Some(ServerMsg::State(s)) = json(&m) {
                state_codec = Some(s.codec);
                continue;
            }
            let Message::Binary(b) = m else { continue };
            if b[0] != proto::KIND_AUDIO {
                continue;
            }
            let f = b[1] & (proto::FLAG_MULAW | proto::FLAG_OPUS);
            if f != flag {
                // frames from before the change
                assert!(audio.is_empty(), "{codec}: flags {f} after the change");
                continue;
            }
            let n = u16::from_le_bytes([b[2], b[3]]) as usize;
            let p = &b[proto::AUDIO_HEADER..];
            match codec {
                "pcm16" => audio.extend(
                    p.as_chunks::<2>()
                        .0
                        .iter()
                        .map(|c| i16::from_le_bytes(*c) as f32 / 32768.0),
                ),
                "mulaw" => {
                    assert_eq!(p.len(), n);
                    audio.extend(p.iter().map(|&u| mulaw_decode(u) as f32 / 32768.0));
                }
                _ => {
                    let mut out = vec![0f32; n];
                    let got = dec.decode(p, n, &mut out).unwrap();
                    assert_eq!(got, n);
                    // Opus at 24 kbit/s narrowband: 60 bytes for 240 samples
                    assert!(p.len() < 100, "{} bytes", p.len());
                    audio.extend_from_slice(&out);
                }
            }
        }
        assert_eq!(state_codec.as_deref(), Some(codec));
        assert!(audio.len() >= 12_000, "{codec}: {} samples", audio.len());
        let f = dominant_hz(&audio[6000..], 12_000.0);
        assert!((f - 1500.0).abs() < 10.0, "{codec}: tone at {f} Hz");
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Packed waterfall lines when asked (`wf`'s `pack`): FLAG_PACKED, smaller
/// than the bins, and unpacking to a line with the tone where it is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn waterfall_lines_pack_when_asked() {
    let (addr, stop) = start().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let req = r#"{"type":"wf","center_hz":7100000,"decim":256,"bins":1024,"fps":20,"pack":true}"#;
    ws.send(Message::Text(req.into())).await.unwrap();
    let (mut packed, mut state_pack) = (0, None);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while packed < 5 && tokio::time::Instant::now() < deadline {
        let m = ws.next().await.unwrap().unwrap();
        if let Some(ServerMsg::State(s)) = json(&m) {
            state_pack = Some(s.wf.pack);
            continue;
        }
        let Message::Binary(b) = m else { continue };
        if b[0] != proto::KIND_WF || b[1] & proto::FLAG_PACKED == 0 {
            continue;
        }
        let bins = u16::from_le_bytes([b[2], b[3]]) as usize;
        let payload = &b[proto::WF_HEADER..];
        assert!(
            payload.len() < bins,
            "{} bytes for {bins} bins",
            payload.len()
        );
        let lv = web888_web::wfpack::unpack(payload, bins).expect("unpacks");
        let f0 = f64::from_le_bytes(b[8..16].try_into().unwrap());
        let df = f64::from_le_bytes(b[16..24].try_into().unwrap());
        if (f0 - (7_100_000.0 - ADC_HZ_HF / 256.0 / 2.0)).abs() > df {
            continue; // the first view's lines
        }
        let k = (0..bins).max_by_key(|&k| lv[k]).unwrap();
        assert!((f0 + k as f64 * df - TONE).abs() <= df, "peak at bin {k}");
        packed += 1;
    }
    assert_eq!(state_pack, Some(true));
    assert!(packed >= 5, "{packed} packed lines");
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn custom_agc_speed_is_kept_and_clamped() {
    let (addr, _) = start().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    // the state the server answers `m` with
    macro_rules! ask {
        ($m:expr) => {{
            ws.send(Message::Text($m.to_string().into())).await.unwrap();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "no state for {}",
                    $m
                );
                if let Some(ServerMsg::State(s)) = json(&ws.next().await.unwrap().unwrap()) {
                    break s;
                }
            }
        }};
    }
    // the state sent on connect first
    while !matches!(
        json(&ws.next().await.unwrap().unwrap()),
        Some(ServerMsg::State(_))
    ) {}
    let s = ask!(r#"{"type":"audio","agc_speed":"slow"}"#);
    assert_eq!(s.agc_speed, proto::AgcSpeed::Slow);
    // medium's by default
    assert_eq!((s.agc_hang_ms, s.agc_decay_db_s), (300.0, 20.0));
    let s = ask!(r#"{"type":"audio","agc_speed":"custom","agc_hang_ms":1200,"agc_decay_db_s":6}"#);
    assert_eq!(s.agc_speed, proto::AgcSpeed::Custom);
    assert_eq!((s.agc_hang_ms, s.agc_decay_db_s), (1200.0, 6.0));
    // out of range: clamped; one alone leaves the other
    let s = ask!(r#"{"type":"audio","agc_hang_ms":99999}"#);
    assert_eq!((s.agc_hang_ms, s.agc_decay_db_s), (5000.0, 6.0));
    let s = ask!(r#"{"type":"audio","agc_decay_db_s":0}"#);
    assert_eq!(s.agc_decay_db_s, 1.0);
}

/// A plain GET: the status line and body.
async fn get(addr: std::net::SocketAddr, path: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut r = String::new();
    s.read_to_string(&mut r).await.unwrap();
    r
}

/// The first `busy` message a new session gets (none: let in).
async fn refused(addr: std::net::SocketAddr) -> Option<String> {
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    match json(&ws.next().await.unwrap().unwrap()) {
        Some(ServerMsg::Busy { msg }) => Some(msg),
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocked_addresses_and_the_daily_limit() {
    let mut c = web888_config::Config::default();
    c.limits.blocked = vec!["127.0.0.0/8".parse().unwrap()];
    let (addr, stop, _) = start_with(c).await;
    let why = refused(addr).await.expect("refused");
    assert!(why.contains("doesn't accept"), "{why}");
    stop.store(true, std::sync::atomic::Ordering::Relaxed);

    // three "minutes" (100 ms each) a day: a session ends when they are
    // used, and the next is refused
    let mut c = web888_config::Config::default();
    c.limits.daily_minutes = 3;
    let (addr, stop, _) = start_with(c).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let (warn, closed) = watch(&mut ws, Duration::from_millis(1500), true).await;
    assert_eq!(closed, Some(web888_web::session::CLOSE_DAILY), "{warn:?}");
    assert!(warn.iter().any(|w| w.contains("in a minute")), "{warn:?}");
    drop(ws);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let why = refused(addr).await.expect("refused");
    assert!(why.contains("today's 3 minutes"), "{why}");
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guests_get_the_open_receivers_and_the_info_page() {
    let mut c = web888_config::Config::default();
    c.auth.listener_hash = Some(web888_web::auth::hash("radio"));
    // no open receivers: the password is needed for everything
    let (addr, stop, _) = start_with(c.clone()).await;
    assert!(get(addr, "/api/info").await.starts_with("HTTP/1.1 401"));
    assert!(
        tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
            .await
            .is_err()
    );
    stop.store(true, std::sync::atomic::Ordering::Relaxed);

    c.limits.open_channels = 1;
    c.kiwi.name = "Test receiver".into();
    c.info.description = "A wire in a tree.".into();
    let (addr, stop, channels) = start_with(c.clone()).await;
    channels.set_policy(web888_core::leases::Policy::from_limits(&c.limits));
    let r = get(addr, "/api/info").await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    for want in [
        "\"name\":\"Test receiver\"",
        "A wire in a tree.",
        "\"open_receivers\":1",
        "\"photo\":false",
        // the privacy section's facts, and the reception notice (on by default)
        "\"reception_notice\":true",
        "\"per_ip\":",
        "\"name\":\"NOAA SWPC\"",
    ] {
        assert!(r.contains(want), "{want}: {r}");
    }
    assert!(get(addr, "/api/auth").await.contains("\"open_channels\":1"));
    assert!(
        get(addr, "/api/info/photo")
            .await
            .starts_with("HTTP/1.1 404")
    );
    // two visitors without the password: the first gets the open
    // receiver, the second waits for it
    let mut lease = vec![];
    let mut keep = vec![];
    for _ in 0..2 {
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
            .await
            .unwrap();
        let mut class = None;
        loop {
            match json(&ws.next().await.unwrap().unwrap()) {
                Some(ServerMsg::Hello { class: c, .. }) => class = Some(c),
                Some(ServerMsg::Lease { granted, .. }) => {
                    lease.push(granted);
                    break;
                }
                _ => {}
            }
        }
        assert_eq!(class.as_deref(), Some("guest"));
        keep.push(ws);
    }
    assert_eq!(lease, [true, false]);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
}
