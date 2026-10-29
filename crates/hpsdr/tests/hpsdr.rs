// SPDX-License-Identifier: GPL-3.0-or-later
//! End to end over loopback UDP: a Protocol 1 client against the server,
//! fed by a simulated radio at 48 kS/s, or at 60 kS/s (resampled).

use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;
use web888_hpsdr::proto::{self, host};
use web888_hpsdr::server::{Config, Control, Server};
use web888_radio::*;
use web888_sim::*;

const F0: f64 = 7_074_000.0;

/// Amplitude of the complex tone at `f` (Blackman-Harris windowed DFT).
fn tone_amp(z: &[(i32, i32)], f: f64, fs: f64) -> f64 {
    let n = z.len();
    let (a0, a1, a2, a3) = (0.35875, 0.48829, 0.14128, 0.01168);
    let (mut re, mut im, mut ws) = (0.0, 0.0, 0.0);
    for (k, &(i, q)) in z.iter().enumerate() {
        let t = 2.0 * std::f64::consts::PI * k as f64 / (n - 1) as f64;
        let w = a0 - a1 * t.cos() + a2 * (2.0 * t).cos() - a3 * (3.0 * t).cos();
        let ph = -2.0 * std::f64::consts::PI * f * k as f64 / fs;
        let (c, s) = (ph.cos(), ph.sin());
        re += w * (i as f64 * c - q as f64 * s);
        im += w * (i as f64 * s + q as f64 * c);
        ws += w;
    }
    (re * re + im * im).sqrt() / ws
}

/// Expected 24-bit amplitude of a tone at `dbfs`, offset `d` from the NCO.
fn want(dbfs: f64, d: f64, r2: u16) -> f64 {
    (1u32 << 22) as f64 * 10f64.powf(dbfs / 20.0) * rx_chain_mag(d, ADC_HZ_HF, r2)
}

fn db(x: f64) -> f64 {
    20.0 * x.log10()
}

#[test]
fn two_receivers_over_udp() {
    two_receivers(10);
}

#[test]
fn two_receivers_from_60k_blocks() {
    two_receivers(8);
}

fn two_receivers(r2: u16) {
    let stop = Arc::new(AtomicBool::new(false));
    let (blk_tx, blk_rx) = mpsc::sync_channel::<Arc<RxBlock>>(32);
    let (ctl_tx, ctl_rx) = mpsc::channel::<Control>();

    // hw thread: simulator at 480 / r2 kS/s, applies tune commands
    let hw_stop = stop.clone();
    let hw = std::thread::spawn(move || {
        let scene = Scene::new()
            .tone(F0 + 1000.0, -20.0)
            .tone(F0 + 9000.0, -30.0);
        let mut r = SimRadio::new(SimConfig::hf(scene, Pacing::Free));
        r.set_rx_rate(r2).unwrap();
        let n = r.caps().rx_channels;
        let mut buf = vec![Iq32::default(); 240 * n];
        let mut first_frame = 0;
        while !hw_stop.load(Ordering::Relaxed) {
            while let Ok(c) = ctl_rx.try_recv() {
                if let Control::Tune { rx, hz } = c {
                    r.tune_rx(rx, hz).unwrap();
                }
            }
            let got = r.read_rx(&mut buf).unwrap();
            let blk = RxBlock {
                first_frame,
                nch: n,
                iq: buf[..got * n].to_vec(),
                overload: false,
            };
            first_frame = blk.end_frame();
            if blk_tx.send(Arc::new(blk)).is_err() {
                break;
            }
        }
    });

    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let cfg = Config {
        board: proto::Board {
            mac: [2, 0, 0, 0x88, 0x88, 1],
            code_version: 1,
            board_id: 1,
        },
        max_rx: 7,
        rx_rate: rx_rate(ADC_HZ_HF, r2),
    };
    let mut server = Server::new(sock, cfg).unwrap();
    let addr = server.local_addr().unwrap();
    let srv_stop = stop.clone();
    let srv = std::thread::spawn(move || {
        server.run(
            blk_rx,
            move |c| {
                let _ = ctl_tx.send(c);
            },
            srv_stop,
        )
    });

    // client
    let c = UdpSocket::bind("127.0.0.1:0").unwrap();
    c.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    c.send_to(&host::discovery(), addr).unwrap();
    let mut buf = [0u8; 2048];
    let (n, _) = c.recv_from(&mut buf).unwrap();
    assert_eq!(n, 60);
    assert_eq!(&buf[0..3], &[0xEF, 0xFE, 0x02]);
    assert_eq!(buf[10], 1, "board id");

    let cc = [host::cc_config(0, 2), host::cc_rx_freq(0, F0 as u32)];
    c.send_to(&host::data(0, cc), addr).unwrap();
    let cc = [
        host::cc_rx_freq(1, (F0 + 5000.0) as u32),
        host::cc_config(0, 2),
    ];
    c.send_to(&host::data(1, cc), addr).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    c.send_to(&host::start_stop(1), addr).unwrap();

    let (mut rx0, mut rx1) = (Vec::new(), Vec::new());
    let mut last_seq = None;
    let mut packets = 0;
    while rx0.len() < 4000 {
        let (n, _) = c.recv_from(&mut buf).expect("EP6 data");
        let (seq, rx) = proto::decode_ep6(&buf[..n], 2).expect("EP6 packet");
        if let Some(s) = last_seq {
            assert_eq!(seq, s + 1, "sequence");
        }
        last_seq = Some(seq);
        packets += 1;
        if packets > 20 {
            // skip the first packets (sent before the tune took effect)
            rx0.extend_from_slice(&rx[0]);
            rx1.extend_from_slice(&rx[1]);
        }
    }
    c.send_to(&host::start_stop(0), addr).unwrap();
    stop.store(true, Ordering::Relaxed);
    srv.join().unwrap();
    hw.join().unwrap();

    let fs = 48_000.0;
    // rx0 tuned to F0: -20 dBFS at +1 kHz, -30 dBFS at +9 kHz
    // rx1 tuned to F0 + 5 kHz: the same tones at -4 kHz and +4 kHz
    for (z, f, dbfs) in [
        (&rx0, 1000.0, -20.0),
        (&rx0, 9000.0, -30.0),
        (&rx1, -4000.0, -20.0),
        (&rx1, 4000.0, -30.0),
    ] {
        let err = db(tone_amp(z, f, fs)) - db(want(dbfs, f, r2));
        assert!(err.abs() < 0.02, "tone at {f}: {err:.3} dB off");
    }
    // nothing where there is no signal
    assert!(db(tone_amp(&rx0, -1000.0, fs)) < db(want(-20.0, 0.0, r2)) - 90.0);
}

/// Maps receiver 0 to channel 3 and receiver 1 to none, and records what
/// the server reports.
struct MapHost(mpsc::Sender<Control>);

impl web888_hpsdr::server::Host for MapHost {
    fn control(&mut self, c: Control) {
        let _ = self.0.send(c);
    }
    fn channel(&mut self, rx: usize) -> Option<usize> {
        (rx == 0).then_some(3)
    }
}

#[test]
fn host_maps_receivers_and_a_silent_client_is_stopped() {
    let stop = Arc::new(AtomicBool::new(false));
    let (blk_tx, blk_rx) = mpsc::sync_channel::<Arc<RxBlock>>(64);
    let (ctl_tx, ctl_rx) = mpsc::channel::<Control>();
    // blocks of 4 channels; channel k carries (k, -k) (24-bit) in every frame
    let feed_stop = stop.clone();
    let feed = std::thread::spawn(move || {
        let mut first_frame = 0;
        while !feed_stop.load(Ordering::Relaxed) {
            let iq = (0..126 * 4i32)
                .map(|n| {
                    let k = n % 4;
                    Iq32 {
                        i: k << 7,
                        q: -k << 7,
                    }
                })
                .collect();
            let blk = RxBlock {
                first_frame,
                nch: 4,
                iq,
                overload: false,
            };
            first_frame = blk.end_frame();
            if blk_tx.send(Arc::new(blk)).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    });

    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let cfg = Config {
        board: proto::Board {
            mac: [2, 0, 0, 0x88, 0x88, 1],
            code_version: 1,
            board_id: 1,
        },
        max_rx: 7,
        rx_rate: 48_000.0,
    };
    let mut server = Server::new(sock, cfg).unwrap();
    let addr = server.local_addr().unwrap();
    let srv_stop = stop.clone();
    let srv = std::thread::spawn(move || server.run(blk_rx, MapHost(ctl_tx), srv_stop));

    let c = UdpSocket::bind("127.0.0.1:0").unwrap();
    c.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let cc = [host::cc_config(0, 2), host::cc_rx_freq(0, F0 as u32)];
    c.send_to(&host::data(0, cc), addr).unwrap();
    c.send_to(&host::start_stop(1), addr).unwrap();
    let mut buf = [0u8; 2048];
    for _ in 0..5 {
        let (n, _) = c.recv_from(&mut buf).expect("EP6 data");
        let (_, rx) = proto::decode_ep6(&buf[..n], 2).expect("EP6 packet");
        assert!(
            rx[0].iter().all(|&s| s == (3, -3)),
            "rx0 from channel 3: {:?}",
            &rx[0][..4]
        );
        assert!(rx[1].iter().all(|&s| s == (0, 0)), "rx1 has no channel");
    }
    let got: Vec<Control> = ctl_rx.try_iter().collect();
    assert_eq!(
        got,
        [Control::Tune { rx: 0, hz: F0 }, Control::Receivers(2)]
    );

    // the client goes silent: stopped after the timeout
    let t0 = std::time::Instant::now();
    let r = ctl_rx
        .recv_timeout(web888_hpsdr::server::CLIENT_TIMEOUT * 2)
        .unwrap();
    assert_eq!(r, Control::Receivers(0));
    assert!(t0.elapsed() >= web888_hpsdr::server::CLIENT_TIMEOUT - Duration::from_millis(500));
    // and no more data after what was in flight
    c.set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    while c.recv_from(&mut buf).is_ok() {}
    std::thread::sleep(Duration::from_millis(100));
    assert!(c.recv_from(&mut buf).is_err(), "stopped streaming");
    stop.store(true, Ordering::Relaxed);
    srv.join().unwrap();
    feed.join().unwrap();
}
