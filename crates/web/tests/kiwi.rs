// SPDX-License-Identifier: GPL-3.0-or-later
//! The Kiwi protocol end to end (`doc/m5-kiwi.md`): simulated radio ->
//! server on a loopback port -> a client sending what kiwirecorder sends.

#![cfg(feature = "kiwi")]

use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;
use web888_hw::{HwLoop, HwStatus};
use web888_radio::*;
use web888_sim::*;
use web888_web::kiwi::adpcm::Adpcm;
use web888_web::proto::RadioInfo;
use web888_web::session::Shared;

/// A tone 1.5 kHz above 7.074 MHz, -40 dBFS.
const TONE: f64 = 7_075_500.0;

async fn start(config: web888_config::Config) -> (std::net::SocketAddr, Arc<AtomicBool>) {
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
        external_spots: Default::default(),
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
        channels,
        pool,
        rx_r2: r2,
        live: tokio::sync::watch::channel(Arc::new(config)).1,
        ips: Default::default(),
        sessions: Default::default(),
        next_session: Default::default(),
        users: Default::default(),
        admin: Default::default(),
        skim: None,
        limit_unit: Duration::from_millis(100),
        kiwi: Default::default(),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = web888_web::router(shared, "/nonexistent".into());
    tokio::spawn(web888_web::serve(listener, app, None));
    (addr, stop)
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn open(addr: std::net::SocketAddr, path: &str) -> Ws {
    tokio_tungstenite::connect_async(format!("ws://{addr}{path}"))
        .await
        .unwrap()
        .0
}

async fn set(ws: &mut Ws, cmd: &str) {
    ws.send(Message::Text(cmd.into())).await.unwrap();
}

/// The next binary frame: its tag and body (kiwiclient's split).
async fn next(ws: &mut Ws) -> Option<(String, Vec<u8>)> {
    loop {
        match tokio::time::timeout(Duration::from_secs(10), ws.next()).await {
            Ok(Some(Ok(Message::Binary(b)))) => {
                return Some((String::from_utf8_lossy(&b[..3]).into(), b[3..].to_vec()));
            }
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Err(_) => return None,
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(e))) => panic!("{e}"),
        }
    }
}

/// MSG name=value pairs until one named `until` (all of them, in order).
async fn msgs_until(ws: &mut Ws, until: &str) -> Vec<(String, String)> {
    let mut v = vec![];
    while let Some((tag, body)) = next(ws).await {
        if tag != "MSG" {
            continue;
        }
        // kiwiclient skips the byte after the tag
        let text = String::from_utf8(body[1..].to_vec()).unwrap();
        let mut done = false;
        for p in text.split(' ') {
            let (k, val) = p.split_once('=').unwrap_or((p, ""));
            done |= k == until;
            v.push((k.to_string(), val.to_string()));
        }
        if done {
            return v;
        }
    }
    panic!("no {until}: {v:?}");
}

fn get<'a>(v: &'a [(String, String)], k: &str) -> Option<&'a str> {
    v.iter().find(|(a, _)| a == k).map(|(_, b)| b.as_str())
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

/// kiwirecorder's start: auth, then on sample_rate its settings.
async fn snd_start(ws: &mut Ws, extra: &[&str]) -> Vec<(String, String)> {
    set(ws, "SET auth t=kiwi p=#").await;
    let m = msgs_until(ws, "sample_rate").await;
    for c in [
        "SET AR OK in=12000 out=44100",
        "SET squelch=0 max=0",
        "SET genattn=0",
        "SET gen=0 mix=-1",
        "SET ident_user=N0CALL",
        "SET mod=usb low_cut=300 high_cut=2700 freq=7074.000",
        "SET agc=1 hang=0 thresh=-100 slope=6 decay=1000 manGain=50",
        "SET keepalive",
    ]
    .iter()
    .chain(extra)
    {
        set(ws, c).await;
    }
    m
}

struct Snd {
    flags: u8,
    seq: u32,
    rssi: f32,
    data: Vec<u8>,
}

async fn next_snd(ws: &mut Ws) -> Snd {
    loop {
        let (tag, b) = next(ws).await.expect("SND");
        if tag == "SND" {
            return Snd {
                flags: b[0],
                seq: u32::from_le_bytes(b[1..5].try_into().unwrap()),
                rssi: 0.1 * u16::from_be_bytes([b[5], b[6]]) as f32 - 127.0,
                data: b[7..].to_vec(),
            };
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kiwirecorder_gets_audio_pcm_adpcm_and_iq() {
    let mut config = web888_config::Config::default();
    config.station.locator = "FN42aa".into();
    config.kiwi.smeter_offset_db = -10.0;
    let (addr, stop) = start(config).await;
    let mut ws = open(addr, "/1234/SND").await;
    let m = snd_start(&mut ws, &["SET compression=0"]).await;
    assert_eq!(get(&m, "badp"), Some("0"));
    assert_eq!(get(&m, "version_maj"), Some("1"));
    assert!(get(&m, "version_min").unwrap().parse::<u32>().unwrap() >= 329);
    assert_eq!(get(&m, "audio_rate"), Some("12000"));
    assert_eq!(get(&m, "freq_offset"), Some("0.000"));
    assert_eq!(get(&m, "bandwidth"), Some("61440000"));
    assert!(get(&m, "sample_rate").unwrap().starts_with("12000."));
    // load_cfg: JSON, URL-encoded, with rx_gps "(lat, lon)" (kiwiclient
    // reads it)
    let cfg: serde_json::Value =
        serde_json::from_str(&urlencoding_decode(get(&m, "load_cfg").unwrap())).unwrap();
    let gps = cfg["rx_gps"].as_str().unwrap();
    let ll: Vec<f64> = gps[1..gps.len() - 1]
        .split(',')
        .map(|s| s.trim().parse().unwrap())
        .collect();
    assert!(
        (ll[0] - 42.02).abs() < 0.01 && (ll[1] + 71.96).abs() < 0.01,
        "{gps}"
    );

    // uncompressed: 512 big-endian samples a packet, the tone at 1.5 kHz
    let mut pcm = vec![];
    let mut last_seq = 0;
    let mut first = true;
    while pcm.len() < 12_000 {
        let p = next_snd(&mut ws).await;
        assert_eq!(p.flags & 0x18, 0, "mono, uncompressed");
        assert_eq!(p.data.len(), 1024);
        if first {
            assert_ne!(p.flags & 0x04, 0, "the first says: new frequency");
            first = false;
        } else {
            assert_eq!(p.seq, last_seq + 1);
        }
        last_seq = p.seq;
        // -40 dBFS, offset -10: -50 dBm
        assert!((p.rssi + 50.0).abs() < 2.0, "{}", p.rssi);
        pcm.extend(
            p.data
                .chunks(2)
                .map(|c| i16::from_be_bytes([c[0], c[1]]) as f32 / 32768.0),
        );
    }
    let f = dominant_hz(&pcm[4000..], 12_000.0);
    assert!((f - 1500.0).abs() < 10.0, "{f}");

    // compressed: 2048 samples in 1024 bytes, the decoder's state kept
    set(&mut ws, "SET compression=1").await;
    let mut dec = Adpcm::default();
    let mut pcm = vec![];
    let mut started = false;
    while pcm.len() < 12_000 {
        let p = next_snd(&mut ws).await;
        if p.flags & 0x10 == 0 {
            assert!(!started);
            continue;
        }
        started = true;
        assert_eq!(p.data.len(), 1024);
        pcm.extend(
            dec.decode(&p.data, -32768, 32767)
                .into_iter()
                .map(|v| v as f32 / 32768.0),
        );
    }
    let f = dominant_hz(&pcm[2048..], 12_000.0);
    assert!((f - 1500.0).abs() < 10.0, "{f}");

    // IQ: stereo, the GPS header, the tone at +1.5 kHz (not -1.5 kHz)
    set(
        &mut ws,
        "SET mod=iq low_cut=-5000 high_cut=5000 freq=7074.000",
    )
    .await;
    let mut iq = vec![];
    while iq.len() < 8192 {
        let p = next_snd(&mut ws).await;
        if p.flags & 0x08 == 0 {
            continue;
        }
        assert_eq!(p.flags & 0x10, 0, "never compressed");
        assert_eq!(p.data.len(), 10 + 2048);
        iq.extend(p.data[10..].chunks(4).map(|c| {
            num_complex::Complex32::new(
                i16::from_be_bytes([c[0], c[1]]) as f32,
                i16::from_be_bytes([c[2], c[3]]) as f32,
            )
        }));
    }
    let n = 4096;
    let mut buf = iq[iq.len() - n..].to_vec();
    rustfft::FftPlanner::new()
        .plan_fft_forward(n)
        .process(&mut buf);
    let k = (0..n)
        .max_by(|&a, &b| buf[a].norm().total_cmp(&buf[b].norm()))
        .unwrap();
    let f = if k < n / 2 {
        k as f64
    } else {
        k as f64 - n as f64
    } * 12_000.0
        / n as f64;
    assert!((f - 1500.0).abs() < 10.0, "{f}");

    // the paired waterfall: zoom 10 (60 kHz) around the tone
    let mut wf = open(addr, "/1234/W/F").await;
    set(&mut wf, "SET auth t=kiwi p=#").await;
    let m = msgs_until(&mut wf, "wf_setup").await;
    assert_eq!(get(&m, "badp"), Some("0"));
    assert_eq!(get(&m, "zoom_max"), Some("14"));
    for c in [
        "SET zoom=10 cf=7075.500000",
        "SET maxdb=-10 mindb=-110",
        "SET wf_speed=4",
        "SET wf_comp=0",
        "SET interp=13",
        "SET keepalive",
    ] {
        set(&mut wf, c).await;
    }
    let m = msgs_until(&mut wf, "zoom").await;
    let start: u32 = get(&m, "start").unwrap().parse().unwrap();
    let mut lines = 0;
    while lines < 5 {
        let (tag, b) = next(&mut wf).await.expect("W/F");
        if tag != "W/F" {
            continue;
        }
        let b = &b[1..];
        assert_eq!(u32::from_le_bytes(b[0..4].try_into().unwrap()), start);
        assert_eq!(u32::from_le_bytes(b[4..8].try_into().unwrap()), 10);
        let seq = u32::from_le_bytes(b[8..12].try_into().unwrap());
        assert!(seq > last_seq, "the audio's sequence: {seq}");
        let px = &b[12..];
        assert_eq!(px.len(), 1024);
        let (k, &peak) = px.iter().enumerate().max_by_key(|&(_, v)| *v).unwrap();
        assert!((k as i64 - 512).abs() <= 2, "peak at {k}");
        // the tone: -40 dBFS in its bin at most, offset -10
        let dbm = peak as f32 - 255.0;
        assert!((-80.0..=-45.0).contains(&dbm), "{dbm}");
        lines += 1;
    }
    drop(wf);
    drop(ws);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_waterfall_alone_and_compressed() {
    let (addr, stop) = start(web888_config::Config::default()).await;
    let mut wf = open(addr, "/99/W/F").await;
    set(&mut wf, "SET auth t=kiwi p=").await;
    msgs_until(&mut wf, "wf_setup").await;
    for c in ["SET zoom=0 start=0", "SET wf_speed=-1", "SET wf_comp=1"] {
        set(&mut wf, c).await;
    }
    let mut lines = 0;
    let mut seqs = vec![];
    while lines < 3 {
        let (tag, b) = next(&mut wf).await.expect("W/F");
        if tag != "W/F" {
            continue;
        }
        let b = &b[1..];
        let flags = u32::from_le_bytes(b[4..8].try_into().unwrap());
        if flags & 0x1_0000 == 0 {
            continue;
        }
        seqs.push(u32::from_le_bytes(b[8..12].try_into().unwrap()));
        assert_eq!(b[12..].len(), (10 + 1024) / 2);
        let px = Adpcm::default().decode(&b[12..], 0, 255);
        assert_eq!(px.len(), 1034);
        // the whole band: noise somewhere between the bounds
        assert!(px[10..].iter().all(|&v| (55..=255).contains(&v)));
        lines += 1;
    }
    // no audio stream: the lines count themselves
    assert!(seqs.windows(2).all(|w| w[1] > w[0]), "{seqs:?}");
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn passwords_and_status() {
    let mut config = web888_config::Config::default();
    config.auth.listener_hash = Some(web888_web::auth::hash("listen1"));
    config.kiwi.name = "Test RX".into();
    let (addr, stop) = start(config).await;

    // no or a wrong password: badp=1 and the stream ends
    let mut ws = open(addr, "/5/SND").await;
    set(&mut ws, "SET auth t=kiwi p=nope").await;
    let m = msgs_until(&mut ws, "badp").await;
    assert_eq!(get(&m, "badp"), Some("1"));
    assert!(next(&mut ws).await.is_none());

    // nothing but keepalives and options before auth
    let mut ws = open(addr, "/6/SND").await;
    set(&mut ws, "SET mod=usb low_cut=300 high_cut=2700 freq=7074").await;
    assert!(next(&mut ws).await.is_none());

    let mut ws = open(addr, "/7/SND").await;
    set(&mut ws, "SET auth t=kiwi p=listen1").await;
    let m = msgs_until(&mut ws, "sample_rate").await;
    assert_eq!(get(&m, "badp"), Some("0"));
    let cfg: serde_json::Value =
        serde_json::from_str(&urlencoding_decode(get(&m, "load_cfg").unwrap())).unwrap();
    assert_eq!(cfg["rx_name"], "Test RX");

    // the same stream again: refused
    let mut dup = open(addr, "/7/SND").await;
    set(&mut dup, "SET auth t=kiwi p=listen1").await;
    let m = msgs_until(&mut dup, "badp").await;
    assert_eq!(get(&m, "badp"), Some("5"));

    let body = reqwest_get(addr, "/status").await;
    assert!(body.contains("auth=password\n"), "{body}");
    assert!(body.contains("name=Test RX\n"));
    assert!(body.contains("users=1\n"), "{body}");
    assert!(body.contains("bands=0-61440000\n"));
    let v: serde_json::Value = serde_json::from_str(&reqwest_get(addr, "/VER").await).unwrap();
    assert_eq!(v["maj"], 1);
    drop(ws);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn off_in_the_config_means_not_found() {
    let mut config = web888_config::Config::default();
    config.kiwi.enabled = false;
    let (addr, stop) = start(config).await;
    assert!(
        tokio_tungstenite::connect_async(format!("ws://{addr}/1/SND"))
            .await
            .is_err()
    );
    let r = raw_get(addr, "/status").await;
    assert!(r.starts_with("HTTP/1.1 404"), "{r}");
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
}

fn urlencoding_decode(s: &str) -> String {
    web888_web::kiwi::wire::url_decode(s)
}

/// A GET over a plain socket: status line, headers and body.
async fn raw_get(addr: std::net::SocketAddr, path: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut b = vec![];
    s.read_to_end(&mut b).await.unwrap();
    String::from_utf8_lossy(&b).into_owned()
}

async fn reqwest_get(addr: std::net::SocketAddr, path: &str) -> String {
    let r = raw_get(addr, path).await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    let body = r.split_once("\r\n\r\n").unwrap().1.to_string();
    // a chunked body: drop the chunk sizes
    if r.to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        let mut out = String::new();
        let mut rest = body.as_str();
        while let Some((len, tail)) = rest.split_once("\r\n") {
            let n = usize::from_str_radix(len.trim(), 16).unwrap_or(0);
            if n == 0 {
                break;
            }
            out.push_str(&tail[..n]);
            rest = &tail[n + 2..];
        }
        return out;
    }
    body
}
