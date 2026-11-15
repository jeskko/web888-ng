// SPDX-License-Identifier: GPL-3.0-or-later
//! The stream decoder over the same stretches of a recording with
//! different tone filters: every signal the skimmer decoded (from
//! `replay`'s output) is decoded again over its whole span, once per
//! filter, from one channel. Writes `OUT.<span>.txt` in `replay`'s
//! format, for a like-for-like score of the filters.
//!
//!     cargo run --release -p web888-fsk --example spans FILE.cf32 RATE CENTRE_HZ REPLAY.txt OUT [SPANS]
//!
//! SPANS: comma-separated Hann lengths in bits, 0 for the boxcar
//! (default 0,1.5).

use num_complex::{Complex32, Complex64};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use web888_cw::dsp;
use web888_fsk::analyze::{FskDecoder, FskParams};

const FS: f64 = 4000.0;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let rate: f64 = a[2].parse().expect("rate");
    let centre: f64 = a[3].parse().expect("centre");
    let spans: Vec<f64> = a
        .get(6)
        .map(|s| s.split(',').map(|v| v.parse().expect("span")).collect())
        .unwrap_or(vec![0.0, 1.5]);
    // signal id -> (kHz, first, last text time); a signal's frequency is
    // that of its lock (text lines carry it too)
    let mut sig: BTreeMap<u32, (f64, f64, f64)> = BTreeMap::new();
    for l in std::fs::read_to_string(&a[4]).expect("replay").lines() {
        if !l.starts_with('T') || l.len() < 25 {
            continue;
        }
        let t: f64 = l[2..9].trim().parse().unwrap();
        let khz: f64 = l[10..19].trim().parse().unwrap();
        let id: u32 = l[20..24].trim().parse().unwrap();
        let e = sig.entry(id).or_insert((khz, t, t));
        e.1 = e.1.min(t);
        e.2 = e.2.max(t);
    }
    let mut raw = Vec::new();
    std::fs::File::open(&a[1])
        .expect("open")
        .read_to_end(&mut raw)
        .expect("read");
    let x: Vec<Complex32> = raw
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
    drop(raw);
    let mut outs: Vec<std::io::BufWriter<std::fs::File>> = spans
        .iter()
        .map(|s| {
            std::io::BufWriter::new(
                std::fs::File::create(format!("{}.{s}.txt", a[5])).expect("out"),
            )
        })
        .collect();
    let decim = (rate / FS).round() as usize;
    let n = ((97.0 * rate / 12_000.0) as usize) | 1;
    let shift = 170.0;
    let taps = dsp::firwin(n, 0.5 * shift + 250.0, rate);
    let dl = (n - 1) as f64 / 2.0 / rate;
    for (&id, &(khz, t0, t1)) in &sig {
        let ts = (t0 - 6.0).max(0.0);
        let i0 = (ts * rate) as usize;
        let i1 = (((t1 + 2.0) * rate) as usize).min(x.len());
        let w = -2.0 * std::f64::consts::PI * (khz * 1e3 - centre) / rate;
        let y: Vec<Complex32> = x[i0..i1]
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let p = Complex64::from_polar(1.0, w * (i0 + i) as f64);
                v * Complex32::new(p.re as f32, p.im as f32)
            })
            .collect();
        let ch: Vec<Complex32> = (0..y.len().saturating_sub(n) / decim)
            .map(|k| {
                y[k * decim..k * decim + n]
                    .iter()
                    .zip(&taps)
                    .map(|(a, &h)| a * h)
                    .sum()
            })
            .collect();
        for (k, &span) in spans.iter().enumerate() {
            let p = FskParams::new(-0.5 * shift, 0.5 * shift, 45.45);
            let mut d = FskDecoder::with_span(FS, p, ts - dl, span);
            for it in d.feed(&ch) {
                if it.0 < t0 - 1.0 {
                    continue;
                }
                let s = it.1.replace('\n', "\\n").replace('\r', "\\r");
                writeln!(outs[k], "T {:7.2} {khz:9.3} {id:4} {s}", it.0).unwrap();
            }
        }
    }
}
