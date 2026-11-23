// SPDX-License-Identifier: GPL-3.0-or-later
//! The page's "what is this?" JSON for audio lo..hi Hz of 12 kHz audio
//! (16-bit PCM WAV, mono), 4.096 s from T0 (fixtures for
//! ui/test/sigdesc.test.ts).
//!
//!     [LSB=1] cargo run --release -p web888-sigid-wasm --example anjson FILE.wav LO HI [T0 [BAND_LO BAND_HI]]

use web888_sigid::measure::NA;
use web888_sigid::scan::{AUDIO_RATE, analyse};
use web888_sigid_wasm::analysis_json;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let f = |i: usize, d: f64| a.get(i).map_or(d, |v| v.parse().expect("number"));
    let b = std::fs::read(&a[1]).expect("read");
    let p = b.windows(4).position(|w| w == b"data").expect("data chunk") + 8;
    let x: Vec<f32> = b[p..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| i16::from_le_bytes(*c) as f32 / 32768.0)
        .collect();
    let s = ((f(4, 0.0) * AUDIO_RATE) as usize).min(x.len().saturating_sub(NA));
    let lsb = std::env::var("LSB").is_ok_and(|v| v == "1");
    let r = analyse(
        &x[s..(s + NA).min(x.len())],
        (f(2, 200.0), f(3, 2800.0)),
        (f(5, 200.0), f(6, 2800.0)),
        lsb,
    );
    println!("{}", analysis_json(r.as_ref()));
}
