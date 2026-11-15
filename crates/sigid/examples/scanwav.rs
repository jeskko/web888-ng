// SPDX-License-Identifier: GPL-3.0-or-later
//! The passband scanner on 12 kHz audio (16-bit PCM WAV, mono, USB): the
//! signals found and what they look like, after the whole file.
//!
//!     [LSB=1] cargo run --release -p web888-sigid --example scanwav FILE.wav [FILE.wav ...]

use web888_sigid::scan::{AUDIO_RATE, Scanner};

fn read_wav(path: &str) -> Vec<f32> {
    let b = std::fs::read(path).expect("read");
    let mut p = 12;
    let mut rate = 0u32;
    while p + 8 <= b.len() {
        let id = &b[p..p + 4];
        let len = u32::from_le_bytes(b[p + 4..p + 8].try_into().unwrap()) as usize;
        if id == b"fmt " {
            rate = u32::from_le_bytes(b[p + 12..p + 16].try_into().unwrap());
        }
        if id == b"data" {
            assert_eq!(rate as f64, AUDIO_RATE, "12 kHz audio only");
            return b[p + 8..(p + 8 + len).min(b.len())]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| i16::from_le_bytes(*c) as f32 / 32768.0)
                .collect();
        }
        p += 8 + len + (len & 1);
    }
    panic!("no data chunk");
}

fn main() {
    for path in std::env::args().skip(1) {
        let x = read_wav(&path);
        let mut sc = Scanner::new();
        sc.set_band(200.0, 3000.0);
        sc.lsb = std::env::var("LSB").is_ok_and(|v| v == "1");
        for c in x.chunks(1200) {
            sc.push(c);
        }
        println!("{path}:");
        for f in sc.found() {
            let top: Vec<String> = f
                .top(3)
                .iter()
                .map(|(c, p)| format!("{c} {:.0}%", 100.0 * p))
                .collect();
            println!(
                "  {:6.0} Hz ({:5.0}-{:5.0}) {:4.0} dB  runs {}  {}",
                f.hz,
                f.lo,
                f.hi,
                f.db,
                f.runs,
                top.join(", ")
            );
        }
    }
}
