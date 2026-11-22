// SPDX-License-Identifier: GPL-3.0-or-later
//! The symbols a station's receiver reads from an IQ recording (cf32,
//! the carrier at 0 Hz), a line a minute, and its decoded minutes:
//!
//!   cargo run --release -p web888-timecode --example symdump -- \
//!       FILE.sigmf-data RATE STATION [CARRIER_OFFSET_HZ]
use web888_timecode::{Event, Receiver, Station};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let fs: f64 = a[2].parse().unwrap();
    let st = Station::from_name(&a[3]).expect("station");
    let off: f64 = a.get(4).map_or(0.0, |s| s.parse().unwrap());
    let b = std::fs::read(&a[1]).unwrap();
    let tone = 1000.0;
    let mut rx = Receiver::new(st, fs, tone, 0.0);
    let mut line = String::new();
    let mut buf = Vec::with_capacity(4096);
    for (n, c) in b.as_chunks::<8>().0.iter().enumerate() {
        let re = f32::from_le_bytes(c[0..4].try_into().unwrap());
        let im = f32::from_le_bytes(c[4..8].try_into().unwrap());
        // upper sideband: the carrier at the tone
        let ph = 2.0 * std::f64::consts::PI * (tone - off) * n as f64 / fs;
        buf.push(re * ph.cos() as f32 - im * ph.sin() as f32);
        if buf.len() == 4096 {
            rx.push(&buf);
            buf.clear();
            for e in rx.poll() {
                match e {
                    Event::Second(s) => {
                        if s.index == Some(0) || line.len() >= 70 {
                            println!("{line}");
                            line = format!("{:8.2} ", s.t);
                        }
                        line.push(s.sym);
                    }
                    Event::Minute(m) => {
                        println!("{line}");
                        line.clear();
                        println!("  minute {} ok {} confirmed {}", m.local, m.ok, m.confirmed);
                    }
                }
            }
        }
    }
    println!("{line}");
}
