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
    let shared = Arc::new(Shared {
        hw: std::sync::Mutex::new(tx),
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
        rx_channel: caps.rx_channels - 1,
        rx_r2: r2,
        live: tokio::sync::watch::channel(Arc::new(web888_config::Config::default())).1,
        busy: Default::default(),
        generation: Default::default(),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = web888_web::router(shared, "/nonexistent".into());
    tokio::spawn(web888_web::serve(listener, app, None));
    (addr, stop)
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
        proto: 1,
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
    assert_eq!(st.hz, 7_074_000.0);
    assert_eq!(st.wf.decim, 2, "full view of 0..61.44 MHz");

    // a second user is turned away
    let (mut ws2, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let m = json(&ws2.next().await.unwrap().unwrap()).unwrap();
    assert!(matches!(m, ServerMsg::Busy { .. }), "{m:?}");

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

    // the user leaves: the next one gets in
    ws.close(None).await.unwrap();
    drop(ws);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (mut ws3, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let m = json(&ws3.next().await.unwrap().unwrap()).unwrap();
    assert!(matches!(m, ServerMsg::Hello { .. }), "{m:?}");
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
