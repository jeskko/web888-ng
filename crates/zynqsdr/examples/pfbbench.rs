// SPDX-License-Identifier: GPL-3.0-or-later
//! The bins ring parser's speed: frames of K selected bins as the
//! channelizer writes them, fed in 256 KiB reads as `ZSDR_PFB_READ` hands
//! them over (60k frames/s); prints the share of a core (CPU time from
//! /proc where it has it). No hardware needed: for the A9.
//!   cargo run --release -p web888-zynqsdr --example pfbbench [K...]
use std::sync::Arc;
use std::time::Instant;
use web888_radio::bins::BinFrames;
use web888_zynqsdr::pfb::BinsParser;

fn cpu_secs() -> Option<f64> {
    let s = std::fs::read_to_string("/proc/self/stat").ok()?;
    let f: Vec<&str> = s[s.rfind(')')? + 2..].split(' ').collect();
    Some((f.get(11)?.parse::<f64>().ok()? + f.get(12)?.parse::<f64>().ok()?) / 100.0)
}

fn main() {
    let ks: Vec<usize> = std::env::args()
        .skip(1)
        .filter_map(|s| s.parse().ok())
        .collect();
    let ks = if ks.is_empty() {
        vec![0, 16, 32, 64]
    } else {
        ks
    };
    let secs = 2.0;
    let frames = (60e3 * secs) as u32;
    for k in ks {
        // one second of ring, replayed (numbers continue)
        let mut p = BinsParser::default();
        p.set_selection(1, Arc::from((0..k).collect::<Vec<_>>()));
        let ring: Vec<u8> = (0..frames)
            .flat_map(|n| {
                let h = 0x5042u64 << 48 | 1 << 44 | 1 << 40 | n as u64;
                std::iter::once(h).chain((0..k as u64).map(move |b| b << 32 | n as u64))
            })
            .flat_map(|w| w.to_le_bytes())
            .collect();
        let mut out: Vec<BinFrames> = Vec::new();
        let (t, c0) = (Instant::now(), cpu_secs());
        let mut got = 0;
        for chunk in ring.chunks(256 * 1024) {
            got += p.push(chunk, &mut out);
            out.clear(); // the reader's consumer drops them
        }
        let dt = t.elapsed().as_secs_f64();
        let used = match (c0, cpu_secs()) {
            (Some(a), Some(b)) => b - a,
            _ => dt,
        };
        println!(
            "{k:2} bins: {got} frames, {:.1} % of a core at 60k frames/s ({:.1} MB/s)",
            100.0 * used / secs,
            ring.len() as f64 / secs / 1e6
        );
    }
}
