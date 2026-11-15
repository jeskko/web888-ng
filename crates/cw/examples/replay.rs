// SPDX-License-Identifier: GPL-3.0-or-later
//! The wide CW skimmer on a capture of a channelizer bin (f32 I, Q pairs
//! at 60 kS/s, e.g. from the browser check's capture), printing each
//! signal's text and how many of its words look run together (10+
//! characters).
//!
//!     [SPACING=hz] cargo run --release -p web888-cw --example replay FILE [neural 0|1]
//!
//! RATE=sps: another input rate (signals within ±0.42 of it); CENTRE=Hz:
//! the input's RF centre, to print RF kHz; EVENTS=1: also every text
//! event as `T time kHz id wpm snr text`; LO=, HI=: a narrower band
//! (Hz from the centre).

use num_complex::Complex32;
use web888_cw::Skimmer;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let b = std::fs::read(&a[1]).expect("read");
    let x: Vec<Complex32> = b
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
    let env = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<f64>().ok());
    let rate = env("RATE").unwrap_or(60_000.0);
    let centre = env("CENTRE").unwrap_or(0.0);
    let events = std::env::var("EVENTS").is_ok();
    let half = if rate == 60_000.0 {
        20_000.0
    } else {
        0.42 * rate
    };
    let mut sk = Skimmer::new_iq(rate, env("LO").unwrap_or(-half), env("HI").unwrap_or(half));
    sk.neural = a.get(2).is_none_or(|v| v == "1");
    if let Some(v) = std::env::var("HI_PCT").ok().and_then(|v| v.parse().ok()) {
        sk.hi_pct = v;
    }
    if let Some(v) = std::env::var("SPACING").ok().and_then(|v| v.parse().ok()) {
        sk.spacing = v;
    }
    let mut text = std::collections::BTreeMap::<u32, (f64, String)>::new();
    for c in x.chunks(2048) {
        sk.push_iq(c);
        for e in sk.poll() {
            if events {
                println!(
                    "T {:7.2} {:9.3} {:4} {:4.1} {:5.1} {}",
                    e.t,
                    (centre + e.hz) / 1e3,
                    e.id,
                    e.wpm,
                    e.snr,
                    e.text
                );
            }
            let t = text.entry(e.id).or_default();
            t.0 = e.hz;
            t.1.push_str(&e.text);
        }
    }
    let (mut words, mut long) = (0, 0);
    for (hz, t) in text.values() {
        if centre > 0.0 {
            println!("{:>9.3} {t}", (centre + hz) / 1e3);
        } else {
            println!("{:>8.0} {t}", hz);
        }
        for w in t.split(' ').filter(|w| !w.is_empty()) {
            words += 1;
            long += usize::from(w.chars().count() >= 10);
        }
    }
    println!("{} signals", text.len());
    println!(
        "{words} words, {long} of 10+ characters ({:.1} %)",
        100.0 * long as f64 / words.max(1) as f64
    );
}
