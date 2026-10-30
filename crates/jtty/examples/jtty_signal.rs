// SPDX-License-Identifier: GPL-3.0-or-later
//! A test recording as raw f32 (12 kHz): messages at audio frequencies
//! and start times, in noise.
//!
//!   cargo run -p web888-jtty --example jtty_signal -- OUT.f32 SECONDS NOISE_RMS \
//!       "TEXT@HZ@START_S" ...

use web888_jtty::{MessageProfile, modulate_message, pack_literal_message};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (out, secs, rms): (&str, f64, f64) = (&a[0], a[1].parse().unwrap(), a[2].parse().unwrap());
    let mut rec = vec![0f32; (secs * 12_000.0) as usize];
    let mut seed = 1u64;
    for v in rec.iter_mut() {
        // uniform noise of the same RMS is enough here
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *v = (rms * 3f64.sqrt() * (2.0 * ((seed >> 11) as f64 / (1u64 << 53) as f64) - 1.0)) as f32;
    }
    for spec in &a[3..] {
        let p: Vec<&str> = spec.split('@').collect();
        let m = pack_literal_message(p[0], MessageProfile::Unknown).unwrap();
        let s = modulate_message(&m.frames, p[1].parse().unwrap(), 0.3).unwrap();
        let start = (p[2].parse::<f64>().unwrap() * 12_000.0) as usize;
        for (i, v) in s.iter().enumerate() {
            if let Some(r) = rec.get_mut(start + i) {
                *r += v;
            }
        }
    }
    let bytes: Vec<u8> = rec.iter().flat_map(|v| v.to_le_bytes()).collect();
    std::fs::write(out, bytes).unwrap();
}
