// SPDX-License-Identifier: GPL-3.0-or-later
//! Decode FT8 WAV files (12 kHz, one slot each) and print jt9-style lines:
//! `HHMMSS snr dt freq ~ message`, then `how pass sync` for scoring.
//!
//!   cargo run --release -p web888-ftx --example ft8wav -- FILES...

use std::time::Instant;
use web888_ftx::{ft8, msg::CallHashes, wav};

fn main() {
    let trace = std::env::var("FTX_TRACE").ok().and_then(|v| v.parse().ok());
    let residual = std::env::var("FTX_RESIDUAL").ok();
    let mut cfg = ft8::Config {
        trace,
        keep_residual: residual.is_some(),
        ..Default::default()
    };
    // Tuning knobs: FTX_SET="llr_scale=4,sync_min=1.3,..."
    for kv in std::env::var("FTX_SET")
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
    {
        let (k, v) = kv.split_once('=').expect("key=value");
        let f: f32 = v.parse().expect("number");
        match k {
            "llr_scale" => cfg.llr_scale = f,
            "sync_min" => cfg.sync_min = f,
            "max_candidates" => cfg.max_candidates = f as usize,
            "passes" => cfg.passes = f as usize,
            "osd_order2" => {
                cfg.osd
                    .get_or_insert(web888_ftx::osd::Depth {
                        order1: 91,
                        order2: 0,
                        order3: 0,
                    })
                    .order2 = f as usize
            }
            "osd_order3" => {
                cfg.osd
                    .get_or_insert(web888_ftx::osd::Depth {
                        order1: 91,
                        order2: 0,
                        order3: 0,
                    })
                    .order3 = f as usize
            }
            "osd_off" => cfg.osd = None,
            "osd_max_distance" => cfg.osd_max_distance = f,
            "osd_max_hard_errors" => cfg.osd_max_hard_errors = f as usize,
            "osd_max_ratio" => cfg.osd_max_ratio = f,
            "osd_max_ratio_known" => cfg.osd_max_ratio_known = f,
            _ => panic!("unknown knob {k}"),
        }
    }
    eprintln!("{cfg:?}");
    let mut dec = ft8::Decoder::new(cfg);
    let mut hashes = CallHashes::default();
    let mut total = 0;
    let mut secs = 0.0;
    for path in std::env::args().skip(1) {
        let path = std::path::Path::new(&path);
        let (x, rate) = wav::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(rate, 12_000, "{}: need 12 kHz", path.display());
        let stem = path.file_stem().unwrap().to_string_lossy();
        let hhmmss = stem.rsplit('_').next().unwrap_or("000000");
        let t = Instant::now();
        let ds = dec.decode(&x, &mut hashes);
        secs += t.elapsed().as_secs_f64();
        if let Some(r) = &residual {
            wav::write(r.as_ref(), dec.residual(), 12_000).unwrap();
        }
        total += ds.len();
        for d in ds {
            println!(
                "{hhmmss} {:3.0} {:4.1} {:4.0} ~  {:<37} {:2} {} {:.1} {:.1} {} {:.1}",
                d.snr,
                d.dt,
                d.freq,
                d.message.text,
                d.how,
                d.pass,
                d.sync,
                d.osd_distance,
                d.hard_errors,
                d.osd_second
            );
        }
    }
    eprintln!("{total} decodes, {secs:.2} s decoding\n{:#?}", dec.stats);
}
