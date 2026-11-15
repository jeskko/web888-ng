// SPDX-License-Identifier: GPL-3.0-or-later
//! The skimmer on a recording: complex f32 IQ (I, Q pairs, little
//! endian) at RATE, e.g. a slice of a HackRF recording. Prints each lock
//! (`L time kHz id baud`) and each text event (`T time kHz id text`, weak
//! characters in lower case for upper and as '_' otherwise), then each
//! signal's text.
//!
//!     cargo run --release -p web888-psk --example replay FILE.cf32 RATE CENTRE_HZ [LO HI]

use num_complex::Complex32;
use std::collections::BTreeMap;
use std::io::Read;
use web888_psk::{Event, Skimmer};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let rate: f64 = a[2].parse().expect("rate");
    let centre: f64 = a[3].parse().expect("centre");
    let lo = a
        .get(4)
        .and_then(|v| v.parse().ok())
        .unwrap_or(-0.42 * rate);
    let hi = a.get(5).and_then(|v| v.parse().ok()).unwrap_or(0.42 * rate);
    let mut sk = Skimmer::new_iq(rate, lo, hi);
    let mut f = std::io::BufReader::new(std::fs::File::open(&a[1]).expect("open"));
    let mut b = vec![0u8; 8 * 4000];
    let mut all: BTreeMap<u32, (f64, f64, String)> = BTreeMap::new();
    let t = std::time::Instant::now();
    loop {
        let mut n = 0;
        while n < b.len() {
            match f.read(&mut b[n..]).expect("read") {
                0 => break,
                k => n += k,
            }
        }
        if n < 8 {
            break;
        }
        let x: Vec<Complex32> = b[..n - n % 8]
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| {
                Complex32::new(
                    f32::from_le_bytes(c[..4].try_into().unwrap()),
                    f32::from_le_bytes(c[4..].try_into().unwrap()),
                )
            })
            .collect();
        sk.push_iq(&x);
        let now = sk.now();
        for e in sk.poll() {
            match e {
                Event::Lock { id, hz, baud } => {
                    println!("L {now:7.2} {:10.4} {id:4} {baud}", (centre + hz) / 1e3);
                    all.insert(id, ((centre + hz) / 1e3, baud, String::new()));
                }
                Event::Text {
                    id,
                    hz,
                    text,
                    t,
                    weak,
                } => {
                    let s: String = text
                        .chars()
                        .zip(weak)
                        .map(|(c, w)| match (w, c.is_ascii_uppercase()) {
                            (false, _) => c,
                            (true, true) => c.to_ascii_lowercase(),
                            (true, false) => '_',
                        })
                        .collect();
                    let s = s.replace('\n', "\\n").replace('\r', "");
                    println!("T {t:7.2} {:10.4} {id:4} {s}", (centre + hz) / 1e3);
                    if let Some(v) = all.get_mut(&id) {
                        v.2.push_str(&s);
                    }
                }
            }
        }
    }
    println!();
    for (id, (khz, baud, s)) in &all {
        if !s.trim().is_empty() {
            println!("{khz:10.4} {baud:6} #{id}: {}", s.trim());
        }
    }
    eprintln!(
        "{:.0} s of IQ in {:.1} s",
        sk.now(),
        t.elapsed().as_secs_f64()
    );
}
