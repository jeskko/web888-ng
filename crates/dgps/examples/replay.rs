// SPDX-License-Identifier: GPL-3.0-or-later
//! The skimmer on a recording: complex f32 IQ (I, Q pairs, little
//! endian), e.g. a 60 kS/s web888 bin (`.sigmf-data`). Prints every lock
//! (`L time kHz id baud`) and message (`M time kHz id type station
//! Z-count health words body`), and a summary per beacon.
//!
//!     cargo run --release -p web888-dgps --example replay FILE RATE CENTRE_HZ [SECONDS]
//!
//! A FILE ending in `.f32` is real audio (12 kS/s mono f32, as the page
//! feeds the skimmer; CENTRE_HZ the USB dial), searched at 300-3000 Hz.

use num_complex::Complex32;
use std::collections::BTreeMap;
use std::io::Read;
use web888_dgps::{Body, Event, Skimmer};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 4 {
        eprintln!("usage: replay FILE RATE CENTRE_HZ [SECONDS]");
        std::process::exit(2);
    }
    let rate: f64 = a[2].parse().expect("rate");
    let centre: f64 = a[3].parse().expect("centre");
    let secs: f64 = a
        .get(4)
        .map(|s| s.parse().expect("seconds"))
        .unwrap_or(f64::INFINITY);
    let edge = 0.42 * rate;
    let audio = a[1].ends_with(".f32");
    let mut sk = if audio {
        Skimmer::new(300.0, 3000.0)
    } else {
        Skimmer::new_iq(rate, -edge, edge)
    };
    let width = if audio { 4 } else { 8 };
    let mut f = std::io::BufReader::new(std::fs::File::open(&a[1]).expect("file"));
    let block = (rate / 10.0) as usize;
    let mut raw = vec![0u8; block * width];
    let mut total = 0usize;
    let t = std::time::Instant::now();
    // per id: kHz, baud, station, messages, complete
    let mut sum: BTreeMap<u32, (f64, f64, u32, u32, u32)> = BTreeMap::new();
    while total as f64 / rate < secs {
        let Ok(()) = f.read_exact(&mut raw) else {
            break;
        };
        if audio {
            let x: Vec<f32> = raw
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c))
                .collect();
            total += x.len();
            sk.push(&x);
        }
        let x: Vec<Complex32> = if audio {
            Vec::new()
        } else {
            raw.as_chunks::<8>()
                .0
                .iter()
                .map(|c| {
                    Complex32::new(
                        f32::from_le_bytes(c[0..4].try_into().unwrap()),
                        f32::from_le_bytes(c[4..8].try_into().unwrap()),
                    )
                })
                .collect()
        };
        if !audio {
            total += x.len();
            sk.push_iq(&x);
        }
        for e in sk.poll() {
            match e {
                Event::Lock { id, hz, baud } => {
                    println!(
                        "L {:7.2} {:8.3} {id:3} {baud}",
                        sk.now(),
                        (centre + hz) / 1e3
                    );
                    sum.insert(id, ((centre + hz) / 1e3, baud, 0, 0, 0));
                }
                Event::Msg { id, hz, msg } => {
                    let body = match &msg.body {
                        Body::None => String::new(),
                        Body::Corr { glonass, sats } => format!(
                            "{} {:?}",
                            if *glonass { "GLONASS" } else { "GPS" },
                            sats.iter().map(|s| s.prn).collect::<Vec<_>>()
                        ),
                        Body::Pos { lat, lon, h } => format!("pos {lat:.5} {lon:.5} {h:.1} m"),
                        Body::Almanac(b) => format!("almanac {b:?}"),
                        Body::Text(t) => format!("text {t:?}"),
                    };
                    let ok = msg.words.iter().filter(|w| w.is_some()).count();
                    println!(
                        "M {:7.2} {:8.3} {id:3} {:2} {:4} {:7.1} {} {}/{} {body}",
                        msg.t,
                        (centre + hz) / 1e3,
                        msg.kind,
                        msg.station,
                        msg.z,
                        msg.health,
                        ok,
                        msg.words.len()
                    );
                    if let Some(s) = sum.get_mut(&id) {
                        s.2 = msg.station;
                        s.3 += 1;
                        s.4 += u32::from(msg.complete());
                    }
                }
            }
        }
    }
    eprintln!(
        "{:.0} s of IQ in {:.1} s",
        total as f64 / rate,
        t.elapsed().as_secs_f64()
    );
    for (id, (k, b, st, n, c)) in sum {
        eprintln!("id {id:3} {k:8.3} kHz {b:3} Bd station {st:4}: {n} messages, {c} complete");
    }
}
