// SPDX-License-Identifier: GPL-3.0-or-later
//! The wide CW skimmer on a capture of a channelizer bin (f32 I, Q pairs
//! at 60 kS/s, e.g. from the browser check's capture), printing each
//! signal's text and how many of its words look run together (10+
//! characters).
//!
//!     [SPACING=hz] cargo run --release -p web888-cw --example replay FILE [neural 0|1]

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
    let mut sk = Skimmer::new_iq(60_000.0, -20_000.0, 20_000.0);
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
            let t = text.entry(e.id).or_default();
            t.0 = e.hz;
            t.1.push_str(&e.text);
        }
    }
    let (mut words, mut long) = (0, 0);
    for (hz, t) in text.values() {
        println!("{:>8.0} {t}", hz);
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
