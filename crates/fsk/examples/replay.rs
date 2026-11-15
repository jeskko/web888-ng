// SPDX-License-Identifier: GPL-3.0-or-later
//! The skimmer on a recording: complex f32 IQ (I, Q pairs, little
//! endian), e.g. a slice of a HackRF recording. Prints every lock and the
//! text of every signal (weak characters in lower case), one line per
//! text event:
//! `T   time  RF_kHz  id  text`, and `L time RF_kHz id params` per lock.
//!
//!     cargo run --release -p web888-fsk --example replay FILE.cf32 RATE CENTRE_HZ [MAX]
//!
//! MAX: the most signals at once (default 64).

use num_complex::Complex32;
use std::io::Read;
use web888_fsk::{Event, Skimmer};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 4 {
        eprintln!("usage: replay FILE.cf32 RATE CENTRE_HZ [MAX]");
        std::process::exit(2);
    }
    let rate: f64 = a[2].parse().expect("rate");
    let centre: f64 = a[3].parse().expect("centre");
    let edge = 0.42 * rate;
    let mut sk = Skimmer::new_iq(rate, -edge, edge);
    sk.max_signals = a.get(4).and_then(|v| v.parse().ok()).unwrap_or(64);
    let mut f = std::io::BufReader::new(std::fs::File::open(&a[1]).expect("open"));
    let mut b = vec![0u8; 8 * 4000];
    let t = std::time::Instant::now();
    loop {
        // whole chunks (a short read would split a sample)
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
                Event::Lock { id, hz, params } => println!(
                    "L {now:7.2} {:9.3} {id:4} {} Bd {:.0} Hz {} {} snr {:.1}",
                    (centre + hz) / 1e3,
                    params.baud,
                    params.shift(),
                    params.framing(),
                    params.charset,
                    params.snr
                ),
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
                        .map(|(c, w)| if w { c.to_ascii_lowercase() } else { c })
                        .collect();
                    println!(
                        "T {t:7.2} {:9.3} {id:4} {}",
                        (centre + hz) / 1e3,
                        s.replace('\n', "\\n").replace('\r', "\\r")
                    )
                }
                Event::Ident { hz, params } => println!(
                    "I {now:7.2} {:9.3}    0 {} Bd {:.0} Hz {}",
                    (centre + hz) / 1e3,
                    params.baud,
                    params.shift(),
                    params.charset
                ),
            }
        }
    }
    eprintln!(
        "{:.0} s of IQ in {:.1} s",
        sk.now(),
        t.elapsed().as_secs_f64()
    );
}
