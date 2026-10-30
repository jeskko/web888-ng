// SPDX-License-Identifier: GPL-3.0-or-later
//! Sensitivity and speed, as jtty-cuf's docs/performance.md measures them:
//! a message at a random start within one frame in 12 kHz noise, SNR in
//! 2500 Hz; "full" is the whole text, "partial" a part of it.
//!
//!   cargo run --release -p web888-jtty --example jtty_bench [-- TRIALS]

use std::time::Instant;
use web888_jtty::{MessageProfile, modulate_message, pack_literal_message, receive_messages};

struct Rng(u64, Option<f64>);

impl Rng {
    fn u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn gauss(&mut self) -> f64 {
        if let Some(s) = self.1.take() {
            return s;
        }
        let u1 = ((self.u64() >> 11) as f64 + 1.0) / 9007199254740993.0;
        let u2 = (self.u64() >> 11) as f64 / 9007199254740992.0;
        let r = (-2.0 * u1.ln()).sqrt();
        let a = 2.0 * std::f64::consts::PI * u2;
        self.1 = Some(r * a.sin());
        r * a.cos()
    }
}

fn main() {
    let trials: usize = std::env::args().nth(1).map_or(40, |a| a.parse().unwrap());
    let rms = 0.1;
    println!("message                     frames   Hz   dB   full partial nothing  (n={trials})");
    let mut audio_s = 0.0;
    let t0 = Instant::now();
    for (text, hz) in [
        ("HELLO", 1500.0),
        ("K1ABC HELLO", 1500.0),
        ("CQ K1ABC FN42", 1500.0),
        ("K1ABC HELLO", 300.0),
        ("K1ABC HELLO", 2800.0),
    ] {
        let m = pack_literal_message(text, MessageProfile::Unknown).unwrap();
        let signal = modulate_message(&m.frames, hz, 1.0).unwrap();
        let power = signal.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / signal.len() as f64;
        for snr in [-12.0, -14.0] {
            let gain = (10f64.powf(snr / 10.0) * rms * rms / 6000.0 * 2500.0 / power).sqrt();
            let (mut full, mut partial) = (0, 0);
            let mut rng = Rng(42, None);
            for _ in 0..trials {
                let start = 12_000 + (rng.u64() % 22_656) as usize;
                let mut rec = vec![0f32; start + signal.len() + 24_000];
                for (i, &v) in signal.iter().enumerate() {
                    rec[start + i] = (gain * v as f64) as f32;
                }
                for v in rec.iter_mut() {
                    *v += (rms * rng.gauss()) as f32;
                }
                audio_s += rec.len() as f64 / 12_000.0;
                let r = receive_messages(&rec).unwrap();
                if r.iter().any(|m| m.text == text) {
                    full += 1;
                } else if r
                    .iter()
                    .any(|m| text.contains(m.text.trim_matches(|c| c == '.' || c == ' ')))
                {
                    partial += 1;
                }
            }
            let pct = |n: usize| 100.0 * n as f64 / trials as f64;
            println!(
                "{text:<27} {:>6} {hz:>4.0} {snr:>4.0} {:>5.1}% {:>6.1}% {:>6.1}%",
                m.frames.len(),
                pct(full),
                pct(partial),
                pct(trials - full - partial)
            );
        }
    }
    let secs = t0.elapsed().as_secs_f64();
    println!(
        "{audio_s:.0} s of audio in {secs:.1} s: {:.1} % of real time (one core)",
        100.0 * secs / audio_s
    );
}
