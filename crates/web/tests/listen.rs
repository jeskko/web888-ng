// SPDX-License-Identifier: GPL-3.0-or-later
//! M3 step 5 end to end: more web users than RX channels. The one left
//! over gets the waterfall only, waits in the queue, sees the others'
//! markers, listens along to one who shares, and gets its own receiver
//! when one frees. Users who opted out can't be followed.

use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use web888_hw::{HwLoop, HwStatus};
use web888_radio::*;
use web888_sim::*;
use web888_web::proto::{self, RadioInfo, ServerMsg, UserMark};
use web888_web::session::Shared;

/// User k's dial; its tone sits `pitch(k)` above (USB audio).
fn dial(k: usize) -> f64 {
    7_000_000.0 + 25_000.0 * k as f64
}

fn pitch(k: usize) -> f64 {
    500.0 + 100.0 * k as f64
}

async fn start(channels: usize, users: usize) -> (String, Arc<AtomicBool>) {
    let mut scene = Scene::new().adc_noise();
    for k in 0..users {
        scene = scene.tone(dial(k) + pitch(k), -40.0);
    }
    let mut cfg = SimConfig::hf(scene, Pacing::RealTime);
    cfg.rx_channels = channels;
    let mut radio = SimRadio::new(cfg);
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
    let mut config = web888_config::Config::default();
    config.limits.per_ip = 0;
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
        },
        channels: web888_core::leases::Channels::new(
            caps.rx_channels,
            web888_core::leases::Policy::default(),
            tx,
        ),
        pool,
        rx_r2: r2,
        live: tokio::sync::watch::channel(Arc::new(config)).1,
        ips: Default::default(),
        sessions: Default::default(),
        next_session: Default::default(),
        users: Default::default(),
        admin: Default::default(),
        skim: None,
        limit_unit: std::time::Duration::from_millis(100),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = web888_web::router(shared, "/nonexistent".into());
    tokio::spawn(web888_web::serve(listener, app, None));
    (format!("ws://{addr}/ws"), stop)
}

struct Client {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    id: u64,
    audio: Vec<f32>,
}

impl Client {
    async fn connect(url: &str) -> Self {
        let (ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        let mut c = Client {
            ws,
            id: 0,
            audio: Vec::new(),
        };
        match c.until(|m| matches!(m, ServerMsg::Hello { .. })).await {
            ServerMsg::Hello { session, .. } => c.id = session,
            _ => unreachable!(),
        }
        c
    }

    async fn send(&mut self, json: String) {
        self.ws.send(Message::Text(json.into())).await.unwrap();
    }

    /// Read until a message matching `f` (audio is collected meanwhile).
    async fn until(&mut self, f: impl Fn(&ServerMsg) -> bool) -> ServerMsg {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let m = tokio::time::timeout_at(deadline, self.ws.next())
                .await
                .expect("timed out waiting for a message")
                .unwrap()
                .unwrap();
            match m {
                Message::Text(t) => {
                    let m: ServerMsg = serde_json::from_str(&t).unwrap();
                    if f(&m) {
                        return m;
                    }
                }
                Message::Binary(b) if b[0] == proto::KIND_AUDIO => self.take_audio(&b),
                _ => {}
            }
        }
    }

    fn take_audio(&mut self, b: &[u8]) {
        self.audio.extend(
            b[proto::AUDIO_HEADER..]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| i16::from_le_bytes(*c) as f32 / 32768.0),
        );
    }

    /// Audio received in the next `t` (after dropping what came before).
    async fn audio_for(&mut self, t: Duration) -> Vec<f32> {
        self.audio.clear();
        let deadline = tokio::time::Instant::now() + t;
        while let Ok(Some(Ok(m))) = tokio::time::timeout_at(deadline, self.ws.next()).await {
            if let Message::Binary(b) = m
                && b[0] == proto::KIND_AUDIO
            {
                self.take_audio(&b);
            }
        }
        std::mem::take(&mut self.audio)
    }
}

/// Frequency of the strongest component of real audio (Hann + FFT).
fn dominant_hz(x: &[f32]) -> f64 {
    let n = (x.len() / 2).next_power_of_two();
    let x = &x[x.len() - n..];
    let mut buf: Vec<num_complex::Complex32> = x
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
    k as f64 * 12_000.0 / n as f64
}

fn is_lease(m: &ServerMsg) -> bool {
    matches!(m, ServerMsg::Lease { .. })
}

fn is_state(m: &ServerMsg) -> bool {
    matches!(m, ServerMsg::State(_))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_thirteenth_listens_along_then_gets_a_receiver() {
    let (url, stop) = start(12, 13).await;
    let mut users = Vec::new();
    for k in 0..12 {
        let mut c = Client::connect(&url).await;
        let m = c.until(is_lease).await;
        assert_eq!(
            m,
            ServerMsg::Lease {
                granted: true,
                position: 0
            },
            "user {k}"
        );
        c.send(format!(r#"{{"type":"tune","hz":{}}}"#, dial(k)))
            .await;
        if k == 3 {
            c.send(r#"{"type":"share","on":false}"#.into()).await;
        }
        c.until(is_state).await;
        users.push(c);
    }

    // the 13th: waterfall only, first in the queue
    let mut last = Client::connect(&url).await;
    let m = last.until(is_lease).await;
    assert_eq!(
        m,
        ServerMsg::Lease {
            granted: false,
            position: 1
        }
    );
    last.send(format!(r#"{{"type":"tune","hz":{}}}"#, dial(12)))
        .await;
    // markers: the others who share (not user 3, not itself)
    let marks: Vec<UserMark> = loop {
        if let ServerMsg::Users { users: u } =
            last.until(|m| matches!(m, ServerMsg::Users { .. })).await
            && u.len() == 11
        {
            break u;
        }
    };
    assert!(marks.iter().all(|m| m.id != users[3].id && m.id != last.id));
    let m5 = marks.iter().find(|m| m.id == users[5].id).unwrap();
    assert_eq!(m5.hz, dial(5));
    assert!(last.audio_for(Duration::from_millis(500)).await.is_empty());

    // an opted-out user can't be followed
    last.send(format!(r#"{{"type":"listen","user":{}}}"#, users[3].id))
        .await;
    let m = last.until(|m| matches!(m, ServerMsg::Error { .. })).await;
    assert!(format!("{m:?}").contains("can't be listened to"), "{m:?}");

    // listen along to user 5: their dial, their audio through our chain
    last.send(format!(r#"{{"type":"listen","user":{}}}"#, users[5].id))
        .await;
    let ServerMsg::State(s) = last.until(is_state).await else {
        unreachable!()
    };
    assert_eq!((s.listen, s.hz), (Some(users[5].id), dial(5)));
    let a = last.audio_for(Duration::from_millis(1500)).await;
    assert!(a.len() > 12_000, "{} samples", a.len());
    assert!(
        (dominant_hz(&a) - pitch(5)).abs() < 10.0,
        "{}",
        dominant_hz(&a)
    );
    // user 5 retunes: the dial follows
    users[5]
        .send(format!(r#"{{"type":"tune","hz":{}}}"#, dial(5) - 200.0))
        .await;
    let ServerMsg::State(s) = last.until(is_state).await else {
        unreachable!()
    };
    assert_eq!(s.hz, dial(5) - 200.0);

    // user 0 leaves: the 13th's receiver is granted; it keeps listening
    // until it switches
    users.remove(0).ws.close(None).await.unwrap();
    let m = last.until(is_lease).await;
    assert_eq!(
        m,
        ServerMsg::Lease {
            granted: true,
            position: 0
        }
    );
    let a = last.audio_for(Duration::from_millis(800)).await;
    let f = dominant_hz(&a);
    assert!(
        (f - pitch(5) - 200.0).abs() < 10.0,
        "{f} Hz from {} samples",
        a.len()
    );
    last.send(r#"{"type":"listen","user":null}"#.into()).await;
    let ServerMsg::State(s) = last.until(is_state).await else {
        unreachable!()
    };
    assert_eq!((s.listen, s.hz), (None, dial(12)));
    let a = last.audio_for(Duration::from_millis(1500)).await;
    assert!(
        (dominant_hz(&a) - pitch(12)).abs() < 10.0,
        "own receiver: {}",
        dominant_hz(&a)
    );
    // with a receiver of its own, listening along is refused
    last.send(format!(r#"{{"type":"listen","user":{}}}"#, users[4].id))
        .await;
    let m = last.until(|m| matches!(m, ServerMsg::Error { .. })).await;
    assert!(format!("{m:?}").contains("you have a receiver"), "{m:?}");
    stop.store(true, Ordering::Relaxed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn listening_ends_when_the_user_opts_out_or_leaves() {
    let (url, stop) = start(1, 2).await;
    let mut a = Client::connect(&url).await;
    a.send(format!(r#"{{"type":"tune","hz":{}}}"#, dial(0)))
        .await;
    a.until(is_state).await;
    let mut b = Client::connect(&url).await;
    assert!(matches!(
        b.until(is_lease).await,
        ServerMsg::Lease { granted: false, .. }
    ));
    b.send(format!(r#"{{"type":"listen","user":{}}}"#, a.id))
        .await;
    let ServerMsg::State(s) = b.until(is_state).await else {
        unreachable!()
    };
    assert_eq!(s.listen, Some(a.id));
    a.send(r#"{"type":"share","on":false}"#.into()).await;
    let m = b.until(|m| matches!(m, ServerMsg::Error { .. })).await;
    assert!(format!("{m:?}").contains("stopped sharing"), "{m:?}");
    let ServerMsg::State(s) = b.until(is_state).await else {
        unreachable!()
    };
    assert_eq!(s.listen, None);
    assert!(b.audio_for(Duration::from_millis(500)).await.is_empty());

    // shared again, followed again; then A leaves: B gets its receiver
    a.send(r#"{"type":"share","on":true}"#.into()).await;
    a.until(is_state).await;
    b.send(format!(r#"{{"type":"listen","user":{}}}"#, a.id))
        .await;
    let ServerMsg::State(s) = b.until(is_state).await else {
        unreachable!()
    };
    assert_eq!(s.listen, Some(a.id));
    a.ws.close(None).await.unwrap();
    drop(a);
    let mut got = (false, false);
    while got != (true, true) {
        match b
            .until(|m| matches!(m, ServerMsg::Error { .. } | ServerMsg::Lease { .. }))
            .await
        {
            ServerMsg::Error { msg } => {
                assert!(msg.contains("left"), "{msg}");
                got.0 = true;
            }
            ServerMsg::Lease { granted, .. } => {
                assert!(granted);
                got.1 = true;
            }
            _ => unreachable!(),
        }
    }
    stop.store(true, Ordering::Relaxed);
}
