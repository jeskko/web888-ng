// SPDX-License-Identifier: GPL-3.0-or-later
//! The page's "what is this?" JSON for audio lo..hi Hz of 12 kHz audio
//! (16-bit PCM WAV, mono), 4.096 s from T0 (fixtures for
//! ui/test/sigdesc.test.ts).
//!
//!     [LSB=1] cargo run --release -p web888-sigid-wasm --example anjson FILE.wav LO HI [T0 [BAND_LO BAND_HI]]
//!
//! or the IQ of a wide look: complex f32 (I, Q) at RATE, Hz from its
//! centre:
//!
//!     cargo run --release -p web888-sigid-wasm --example anjson FILE.cf32 LO HI RATE BAND_LO BAND_HI

use num_complex::Complex32;
use web888_sigid::measure::NA;
use web888_sigid::measure::iq_len;
use web888_sigid::scan::{AUDIO_RATE, analyse, analyse_iq};
use web888_sigid_wasm::analysis_json;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let f = |i: usize, d: f64| a.get(i).map_or(d, |v| v.parse().expect("number"));
    let b = std::fs::read(&a[1]).expect("read");
    if a[1].ends_with(".cf32") {
        let rate = f(4, 24000.0);
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
        let x = &x[..iq_len(rate).min(x.len())];
        let r = analyse_iq(
            x,
            rate,
            (f(2, -5000.0), f(3, 5000.0)),
            (f(5, -9900.0), f(6, 9900.0)),
        );
        println!("{}", analysis_json(r.as_ref()));
        return;
    }
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
