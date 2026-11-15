// SPDX-License-Identifier: GPL-3.0-or-later
//! A test recording: complex f32 IQ at RATE with PSK31 and PSK63 signals
//! (repeating text) at +1000 and +1700 Hz, in noise, SECS long; for
//! playing through `web888d --backend iq --iq file:...` to the page.
//!
//!     cargo run --release -p web888-psk --example synthiq OUT.cf32 [RATE] [SECS] [audio]
//!
//! `audio`: real f32 audio at RATE instead (e.g. 12000, for the page's
//! decoder worker), the signals at 1000 and 1700 Hz.

use std::io::Write;
use web888_cw::dsp::Rng;
use web888_psk::synth;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let rate: f64 = a.get(2).and_then(|v| v.parse().ok()).unwrap_or(480_000.0);
    let secs: f64 = a.get(3).and_then(|v| v.parse().ok()).unwrap_or(60.0);
    let n = (secs * rate) as usize;
    let mut rng = Rng::new(7);
    let sigs = [
        (31.25, 1000.0, "CQ CQ de N0CALL N0CALL pse k\n", 0.01f32),
        (
            62.5,
            1700.0,
            "N0CALL de N1CALL ur rst 599 599 name Al qth Boston\n",
            0.006,
        ),
    ];
    // each signal's envelope at the output rate, its text repeated to fill
    let bases: Vec<(f64, Vec<f32>)> = sigs
        .iter()
        .map(|&(baud, hz, text, amp)| {
            let per = text.chars().count() as f64 * 10.0 / baud; // ~10 bits a character
            let reps = (secs / per).ceil() as usize + 1;
            let b = synth::baseband(&synth::symbols(&text.repeat(reps)), baud, rate);
            (hz, b.into_iter().map(|v| v * amp).collect())
        })
        .collect();
    let audio = a.get(4).is_some_and(|v| v == "audio");
    let mut f = std::io::BufWriter::new(std::fs::File::create(&a[1]).expect("create"));
    let sd = 0.002;
    for i in 0..n {
        let (mut re, mut im) = ((sd * rng.normal()) as f32, (sd * rng.normal()) as f32);
        for (hz, b) in &bases {
            let v = b.get(i).copied().unwrap_or(0.0);
            let p = 2.0 * std::f64::consts::PI * hz * i as f64 / rate;
            re += v * p.cos() as f32;
            im += v * p.sin() as f32;
        }
        f.write_all(&re.to_le_bytes()).unwrap();
        if !audio {
            f.write_all(&im.to_le_bytes()).unwrap();
        }
    }
}
