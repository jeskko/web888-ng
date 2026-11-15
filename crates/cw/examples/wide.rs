// SPDX-License-Identifier: GPL-3.0-or-later
//! The wide skimmer's cost: 24 CW stations (half of them weak) in 40 s of
//! a 60 kS/s channelizer bin: classical, hybrid (the network only where
//! the classical decoder isn't clean above 12 dB) and neural (the default).
//!
//!     cargo run --release -p web888-cw --example wide

use num_complex::Complex32;
use std::time::Instant;
use web888_cw::Skimmer;
use web888_cw::dsp::Rng;
use web888_cw::synth::{self, Keying};

fn main() {
    let rate = 60_000.0;
    let mut rng = Rng::new(5);
    let n = (40.0 * rate) as usize;
    let sd = 0.003f32;
    let mut x: Vec<Complex32> = (0..n)
        .map(|_| Complex32::new(sd * rng.normal() as f32, sd * rng.normal() as f32))
        .collect();
    for j in 0..24 {
        let hz = -19_000.0 + j as f64 * 1650.0;
        let wpm = 14.0 + (j % 6) as f64 * 5.0;
        // half strong (~+15 dB in 2500 Hz), half weak (~-3 dB)
        let amp = if j % 2 == 0 { 0.05 } else { 0.008 };
        let k = Keying {
            lead: 0.5,
            tail: 1.0,
            jitter: 0.05,
            ..Keying::new(wpm)
        };
        let key = synth::keying(
            &format!("CQ DE N{}CALL N{}CALL K", j % 10, j % 10),
            &k,
            rate,
            &mut rng,
        );
        let ph0 = rng.uniform() * std::f64::consts::TAU;
        for (i, v) in x.iter_mut().enumerate() {
            let a = key[i % key.len()] * amp;
            let p = 2.0 * std::f64::consts::PI * hz * i as f64 / rate + ph0;
            *v += Complex32::new(a * p.cos() as f32, a * p.sin() as f32);
        }
    }
    // WIDE_DUMP=path: the IQ as f32 I, Q pairs (for timing the wasm build)
    if let Ok(path) = std::env::var("WIDE_DUMP") {
        let b: Vec<u8> = x
            .iter()
            .flat_map(|v| [v.re.to_le_bytes(), v.im.to_le_bytes()])
            .flatten()
            .collect();
        std::fs::write(path, b).expect("write");
    }
    for (name, neural, above) in [
        ("classical", false, f64::INFINITY),
        ("hybrid (classical above 12 dB)", true, 12.0),
        ("neural (the default)", true, f64::INFINITY),
    ] {
        let mut sk = Skimmer::new_iq(rate, -20_000.0, 20_000.0);
        sk.neural = neural;
        sk.classical_above = above;
        if let Some(v) = std::env::var("SPACING").ok().and_then(|v| v.parse().ok()) {
            sk.spacing = v;
        }
        if let Some(v) = std::env::var("HI_PCT").ok().and_then(|v| v.parse().ok()) {
            sk.hi_pct = v;
        }
        let mut text = std::collections::HashMap::<u32, String>::new();
        let t = Instant::now();
        for c in x.chunks(2048) {
            sk.push_iq(c);
            for e in sk.poll() {
                text.entry(e.id).or_default().push_str(&e.text);
            }
        }
        let el = t.elapsed().as_secs_f64();
        let calls = text.values().filter(|t| t.contains("CALL N")).count();
        println!(
            "{name:<31} {:>5.1} % of a core, {} signals, {} with a call read twice",
            100.0 * el / 40.0,
            sk.active().len(),
            calls
        );
    }
}
