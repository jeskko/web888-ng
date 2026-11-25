// SPDX-License-Identifier: GPL-3.0-or-later
//! "Label every signal" offline: 60 kS/s bins cut out of a wideband
//! HackRF recording (cs8, interleaved i8), the way the page gets them
//! from the channelizer, each skimmed as the page does (`skim::Window`:
//! the signals in ±20 kHz, each identified). Prints every signal and
//! the time each bin took.
//!
//!     cargo run --release -p web888-sigid --example skimcs8 REC.cs8 RATE CENTRE_HZ T0_S LO_KHZ HI_KHZ
//!
//! Bins every 30 kHz from LO to HI (their centres), as the page hops.

use num_complex::Complex32;
use rustfft::FftPlanner;
use std::io::{Read, Seek, SeekFrom};
use std::time::Instant;
use web888_sigid::measure::iq_len;
use web888_sigid::skim::Window;

const BIN_RATE: f64 = 60_000.0;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 7 {
        eprintln!("usage: skimcs8 REC.cs8 RATE CENTRE_HZ T0_S LO_KHZ HI_KHZ");
        std::process::exit(2);
    }
    let rate: f64 = a[2].parse().unwrap();
    let centre: f64 = a[3].parse().unwrap();
    let t0: f64 = a[4].parse().unwrap();
    let (lo, hi): (f64, f64) = (
        a[5].parse::<f64>().unwrap() * 1e3,
        a[6].parse::<f64>().unwrap() * 1e3,
    );
    let n = iq_len(rate);
    let mut f = std::fs::File::open(&a[1]).expect("open");
    f.seek(SeekFrom::Start(2 * (t0 * rate) as u64)).unwrap();
    let mut b = vec![0u8; 2 * n];
    f.read_exact(&mut b).expect("4.096 s from T0");
    let mut big: Vec<Complex32> = b
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| Complex32::new(c[0] as i8 as f32 / 128.0, c[1] as i8 as f32 / 128.0))
        .collect();
    FftPlanner::new().plan_fft_forward(n).process(&mut big);
    let m = iq_len(BIN_RATE);
    let df = rate / n as f64;
    let inv = FftPlanner::new().plan_fft_inverse(m);
    let (mut signals, mut secs, mut bins) = (0usize, 0f64, 0usize);
    let mut c = lo;
    while c <= hi + 1.0 {
        // the bin: 60 kHz of the recording's spectrum around c, back to time
        let k0 = ((c - centre) / df).round() as i64 - (m / 2) as i64;
        let mut y: Vec<Complex32> = (0..m)
            .map(|j| big[(k0 + j as i64).rem_euclid(n as i64) as usize])
            .collect();
        y.rotate_left(m / 2);
        inv.process(&mut y);
        let g = 1.0 / n as f32;
        y.iter_mut().for_each(|v| *v *= g);
        let t = Instant::now();
        let w = Window::new(&y, BIN_RATE).expect("a window");
        let r = w.regions(-20_000.0, 20_000.0, 48);
        let mut lines = Vec::new();
        let mut sorted = r.clone();
        sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (l, h, db) in &sorted {
            let top = w.identify(0.5 * (l + h));
            lines.push(format!(
                "  {:10.2} kHz {:5.0} Hz {:3.0} dB  {:>12} {:.2}  {:>12} {:.2}",
                (c + 0.5 * (l + h)) / 1e3,
                h - l,
                db,
                top[0].0,
                top[0].1,
                top[1].0,
                top[1].1
            ));
        }
        let s = t.elapsed().as_secs_f64();
        println!("bin {:.0} kHz: {} signals, {:.2} s", c / 1e3, r.len(), s);
        for l in lines {
            println!("{l}");
        }
        signals += r.len();
        secs += s;
        bins += 1;
        c += 30_000.0;
    }
    println!(
        "{bins} bins, {signals} signals, {secs:.1} s ({:.3} s a signal)",
        secs / signals.max(1) as f64
    );
}
