// SPDX-License-Identifier: GPL-3.0-or-later
//! The iq backend's channelizer (`doc/m6-channelizer.md` step 2): 7.68 MS/s
//! noise through `IqRadio` in bins mode, N bins selected and a power line
//! every 3000 frames (20 a second); prints the speed against real time.
//!   cargo run --release -p web888-iq --example binsbench [bins]
use num_complex::Complex32;
use std::io;
use std::time::Instant;
use web888_iq::source::IqSource;
use web888_iq::{IqConfig, IqRadio};
use web888_radio::bins::BinRadio;

struct Noise(u32);
impl IqSource for Noise {
    fn read(&mut self, buf: &mut [Complex32]) -> io::Result<usize> {
        for o in buf.iter_mut() {
            self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
            let a = (self.0 >> 24) as i8 as f32 / 128.0;
            let b = ((self.0 >> 16) & 0xff) as u8 as i8 as f32 / 128.0;
            *o = Complex32::new(a, b);
        }
        Ok(buf.len())
    }
}

fn main() {
    let nbins: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(64);
    let mut cfg = IqConfig::new(8.6e6, 7.68e6);
    cfg.bins = true;
    let mut r = IqRadio::direct(cfg, Box::new(Noise(1))).unwrap();
    let caps = r.bin_caps();
    let sel: Vec<usize> = (0..nbins).map(|k| k * caps.bins / nbins.max(1)).collect();
    r.set_bins(&sel).unwrap();
    r.set_power(3000).unwrap();
    let secs = 5.0;
    let (mut frames, mut lines, mut out, mut line) = (0usize, 0, Vec::new(), Vec::new());
    let t = Instant::now();
    // until `secs` of frames, or of lines (no bins selected: no frames)
    while frames < (caps.frame_rate * secs) as usize && (lines as f64) < 20.0 * secs {
        out.clear();
        frames += r.read_bins(&mut out).unwrap();
        while r.read_power(&mut line).unwrap().is_some() {
            lines += 1;
        }
    }
    let dt = t.elapsed().as_secs_f64();
    println!(
        "{} bins of {}, {frames} frames, {lines} power lines in {dt:.2} s: {:.1}x real time",
        sel.len(),
        caps.bins,
        secs / dt
    );
}
