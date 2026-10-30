// SPDX-License-Identifier: GPL-3.0-or-later
//! Decodes a raw f32 recording (12 kHz mono, e.g. from jtty_signal) as a
//! stream and prints each message as it completes, then the time taken.
//!
//!   jtty_decode FILE.f32

use std::time::Instant;
use web888_jtty::Receiver;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: jtty_decode FILE.f32");
    let bytes = std::fs::read(path).unwrap();
    let pcm: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&b| f32::from_le_bytes(b))
        .collect();
    let t0 = Instant::now();
    let mut r = Receiver::new();
    let mut fed = 0;
    for piece in pcm.chunks(1200) {
        fed += piece.len();
        for m in r.push(piece) {
            println!(
                "{:6.1} s  {:7.3} s  {:4} Hz  {}",
                fed as f64 / 12_000.0,
                m.start_sample as f64 / 12_000.0,
                m.frequency_hz,
                m.text
            );
        }
    }
    for m in r.finish() {
        println!(
            "   end  {:7.3} s  {:4} Hz  {}",
            m.start_sample as f64 / 12_000.0,
            m.frequency_hz,
            m.text
        );
    }
    let secs = t0.elapsed().as_secs_f64();
    let audio = pcm.len() as f64 / 12_000.0;
    println!(
        "{audio:.0} s of audio in {secs:.1} s: {:.0} % of real time",
        100.0 * secs / audio
    );
}
