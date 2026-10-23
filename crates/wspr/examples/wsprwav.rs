// SPDX-License-Identifier: GPL-3.0-or-later
//! Decode WSPR-2 WAV files (12 kHz, one 2-minute slot each) and print
//! wsprd-style lines: `HHMM snr dt freq drift message`.
//!
//!   cargo run --release -p web888-wspr --example wsprwav -- DIAL_MHZ FILES...

use std::time::Instant;
use web888_ftx::wav;
use web888_wspr::{Config, Decoder};

fn main() {
    let mut args = std::env::args().skip(1);
    let dial: f64 = args.next().expect("dial MHz").parse().expect("dial MHz");
    let mut cfg = Config::default();
    if let Some(c) = std::env::var("WSPR_CYCLES")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        cfg.maxcycles = c;
    }
    let mut dec = Decoder::new(cfg);
    let mut secs = 0.0;
    for path in args {
        let path = std::path::Path::new(&path);
        let (x, rate) = wav::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(rate, 12_000, "{}: need 12 kHz", path.display());
        let stem = path.file_stem().unwrap().to_string_lossy();
        let hhmm = &stem.rsplit('_').next().unwrap_or("000000")[..4];
        let t = Instant::now();
        let ds = dec.decode(&x);
        secs += t.elapsed().as_secs_f64();
        for d in ds {
            println!(
                "{hhmm} {:3.0} {:4.1} {:10.6} {:2}  {} ",
                d.snr,
                d.dt,
                dial + d.freq / 1e6,
                d.drift as i32,
                d.message.text
            );
        }
    }
    eprintln!("{secs:.2} s decoding");
}
