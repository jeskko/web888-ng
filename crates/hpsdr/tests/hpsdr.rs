// SPDX-License-Identifier: GPL-3.0-or-later
//! End to end over loopback UDP: a Protocol 1 client against the server,
//! fed by a simulated radio at 48 kS/s.

use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;
use web888_hpsdr::proto::{self, host};
use web888_hpsdr::server::{Block, Config, Control, Server};
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
fn want(dbfs: f64, d: f64) -> f64 {
    (1u32 << 22) as f64 * 10f64.powf(dbfs / 20.0) * rx_chain_mag(d, ADC_HZ_HF, 10)
}

fn db(x: f64) -> f64 {
    20.0 * x.log10()
}

#[test]
fn two_receivers_over_udp() {
    let stop = Arc::new(AtomicBool::new(false));
    let (blk_tx, blk_rx) = mpsc::sync_channel::<Block>(32);
    let (ctl_tx, ctl_rx) = mpsc::channel::<Control>();

    // hw thread: simulator at 48 kS/s, applies tune commands
    let hw_stop = stop.clone();
    let hw = std::thread::spawn(move || {
        let scene = Scene::new()
            .tone(F0 + 1000.0, -20.0)
            .tone(F0 + 9000.0, -30.0);
        let mut r = SimRadio::new(SimConfig::hf(scene, Pacing::Free));
        r.set_rx_rate(10).unwrap();
        let n = r.caps().rx_channels;
        let mut buf = vec![Iq32::default(); 240 * n];
        while !hw_stop.load(Ordering::Relaxed) {
            while let Ok(c) = ctl_rx.try_recv() {
                if let Control::Tune { rx, hz } = c {
                    r.tune_rx(rx, hz).unwrap();
                }
            }
            let got = r.read_rx(&mut buf).unwrap();
            let iq = buf[..got * n].iter().map(|s| (s.i, s.q)).collect();
            if blk_tx
                .send(Block {
                    nch: n,
                    iq,
                    overload: false,
                })
                .is_err()
            {
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
    };
    let mut server = Server::new(sock, cfg).unwrap();
    let addr = server.local_addr().unwrap();
    let srv_stop = stop.clone();
    let srv = std::thread::spawn(move || server.run(blk_rx, ctl_tx, srv_stop));

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
        let err = db(tone_amp(z, f, fs)) - db(want(dbfs, f));
        assert!(err.abs() < 0.02, "tone at {f}: {err:.3} dB off");
    }
    // nothing where there is no signal
    assert!(db(tone_amp(&rx0, -1000.0, fs)) < db(want(-20.0, 0.0)) - 90.0);
}
