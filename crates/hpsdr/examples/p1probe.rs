// SPDX-License-Identifier: GPL-3.0-or-later
//! Minimal HPSDR Protocol 1 client: tune receivers, stream for a while and
//! report the strongest tones per receiver. For checking a server (web888d
//! on a unit or in the emulator) without a full SDR program.
//!
//!   cargo run --release -p web888-hpsdr --example p1probe -- \
//!       HOST:PORT [--secs S] [--rate 48|96|192|384] FREQ_HZ...
//!
//! Prints, per receiver: sample count, sequence gaps, RMS in dBFS and the
//! three strongest tones (offset from the tuned frequency, dBFS).

use num_complex::Complex;
use rustfft::FftPlanner;
use std::net::{ToSocketAddrs, UdpSocket};
use std::process::exit;
use std::time::{Duration, Instant};
use web888_hpsdr::proto::{self, host};

const FULL: f64 = (1u32 << 23) as f64; // 24-bit full scale

fn usage() -> ! {
    eprintln!("usage: p1probe HOST:PORT [--secs S] [--rate 48|96|192|384] FREQ_HZ...");
    exit(2)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let addr = args
        .next()
        .and_then(|a| a.to_socket_addrs().ok()?.next())
        .unwrap_or_else(|| usage());
    let (mut secs, mut khz, mut freqs) = (3.0f64, 48u32, Vec::<u32>::new());
    while let Some(a) = args.next() {
        match a.as_str() {
            "--secs" => {
                secs = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| usage())
            }
            "--rate" => {
                khz = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| usage())
            }
            f => freqs.push(f.parse::<f64>().unwrap_or_else(|_| usage()) as u32),
        }
    }
    let speed = match khz {
        48 => 0,
        96 => 1,
        192 => 2,
        384 => 3,
        _ => usage(),
    };
    let nrx = freqs.len();
    if nrx == 0 || nrx > proto::MAX_RX {
        usage();
    }
    let fs = khz as f64 * 1000.0;

    let sock = UdpSocket::bind("0.0.0.0:0").expect("bind");
    sock.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut buf = [0u8; 2048];

    sock.send_to(&host::discovery(), addr).expect("send");
    match sock.recv_from(&mut buf) {
        Ok((60, _)) if buf[0..2] == [0xEF, 0xFE] => println!(
            "discovery: board id {}, code version {}, {}",
            buf[10],
            buf[9],
            if buf[2] == 3 { "busy" } else { "idle" }
        ),
        Ok((n, _)) => println!("discovery: unexpected {n}-byte reply"),
        Err(e) => {
            eprintln!("discovery: no reply ({e})");
            exit(1)
        }
    }

    // configuration and frequencies, two C&C words per packet
    let mut cc = vec![host::cc_config(speed, nrx)];
    cc.extend(
        freqs
            .iter()
            .enumerate()
            .map(|(r, &f)| host::cc_rx_freq(r, f)),
    );
    if cc.len() % 2 == 1 {
        cc.push(host::cc_config(speed, nrx));
    }
    let mut seq = 0u32;
    let mut send_cc = |sock: &UdpSocket| {
        for pair in cc.chunks(2) {
            sock.send_to(&host::data(seq, [pair[0], pair[1]]), addr)
                .unwrap();
            seq += 1;
        }
    };
    send_cc(&sock);
    sock.send_to(&host::start_stop(1), addr).unwrap();

    let mut rx: Vec<Vec<(i32, i32)>> = vec![Vec::new(); nrx];
    let (mut packets, mut gaps, mut last) = (0u64, 0u64, None::<u32>);
    let t0 = Instant::now();
    let mut keep = Instant::now();
    while t0.elapsed().as_secs_f64() < secs {
        if keep.elapsed() > Duration::from_millis(100) {
            send_cc(&sock); // clients stream EP2 continuously; this is enough
            keep = Instant::now();
        }
        let n = match sock.recv_from(&mut buf) {
            Ok((n, _)) => n,
            Err(e) => {
                eprintln!("no data ({e})");
                break;
            }
        };
        let Some((s, data)) = proto::decode_ep6(&buf[..n], nrx) else {
            continue;
        };
        if let Some(l) = last
            && s != l.wrapping_add(1)
        {
            gaps += 1;
        }
        last = Some(s);
        packets += 1;
        // skip the first ~0.25 s (settling, frequencies taking effect)
        if t0.elapsed() > Duration::from_millis(250) {
            for (o, d) in rx.iter_mut().zip(data) {
                o.extend(d);
            }
        }
    }
    sock.send_to(&host::start_stop(0), addr).unwrap();
    let dt = t0.elapsed().as_secs_f64();
    println!(
        "{packets} packets in {dt:.1} s ({:.0} samples/s per rx), {gaps} sequence gaps",
        packets as f64 * proto::slots(nrx) as f64 * 2.0 / dt
    );

    let mut planner = FftPlanner::<f64>::new();
    for (r, z) in rx.iter().enumerate() {
        let n = 1usize << (z.len().max(2).ilog2()).min(16);
        if n < 1024 {
            println!("rx{r}: {} samples, too few", z.len());
            continue;
        }
        let rms = (z
            .iter()
            .map(|&(i, q)| (i as f64).powi(2) + (q as f64).powi(2))
            .sum::<f64>()
            / z.len() as f64)
            .sqrt();
        // Blackman-Harris window, amplitude-normalised
        let (a0, a1, a2, a3) = (0.35875, 0.48829, 0.14128, 0.01168);
        let w: Vec<f64> = (0..n)
            .map(|k| {
                let t = 2.0 * std::f64::consts::PI * k as f64 / n as f64;
                a0 - a1 * t.cos() + a2 * (2.0 * t).cos() - a3 * (3.0 * t).cos()
            })
            .collect();
        let ws: f64 = w.iter().sum();
        let mut x: Vec<Complex<f64>> = z[z.len() - n..]
            .iter()
            .zip(&w)
            .map(|(&(i, q), &w)| Complex::new(i as f64 * w, q as f64 * w))
            .collect();
        planner.plan_fft_forward(n).process(&mut x);
        let mag: Vec<f64> = x.iter().map(|c| c.norm() / ws / FULL).collect();
        let mut idx: Vec<usize> = (0..n).collect();
        idx.sort_by(|&a, &b| mag[b].total_cmp(&mag[a]));
        let mut tones: Vec<usize> = Vec::new();
        for i in idx {
            if tones.iter().all(|&t| {
                (t as isize - i as isize)
                    .unsigned_abs()
                    .min(n - (t as isize - i as isize).unsigned_abs())
                    > 8
            }) {
                tones.push(i);
            }
            if tones.len() == 3 {
                break;
            }
        }
        let fmt = tones
            .iter()
            .map(|&i| {
                let f = if i < n / 2 {
                    i as f64
                } else {
                    i as f64 - n as f64
                } * fs
                    / n as f64;
                format!("{f:+.1} Hz {:.1} dBFS", 20.0 * mag[i].log10())
            })
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "rx{r} @ {} Hz: {} samples, rms {:.1} dBFS; tones {fmt}",
            freqs[r],
            z.len(),
            20.0 * (rms / FULL).log10()
        );
    }
}
