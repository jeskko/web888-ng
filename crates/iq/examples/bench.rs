// SPDX-License-Identifier: GPL-3.0-or-later
//! Channelizer throughput: N RX channels at 48 kS/s from 7.68 MS/s input.
//!   cargo run --release -p web888-iq --example bench [channels]
use num_complex::Complex32;
use std::io;
use std::time::Instant;
use web888_iq::source::IqSource;
use web888_iq::{IqConfig, IqRadio};
use web888_radio::*;

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
    let chans: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(7);
    let mut r = IqRadio::direct(IqConfig::new(10.5e6, 7.68e6), Box::new(Noise(1))).unwrap();
    r.set_rx_rate(10).unwrap();
    for c in 0..chans {
        r.tune_rx(c, 8e6 + 0.5e6 * c as f64).unwrap();
    }
    let n = r.caps().rx_channels;
    let mut buf = vec![Iq32::default(); 4800 * n];
    let secs = 5.0;
    let t = Instant::now();
    let mut frames = 0;
    while frames < (48_000.0 * secs) as usize {
        frames += r.read_rx(&mut buf).unwrap();
    }
    let dt = t.elapsed().as_secs_f64();
    println!(
        "{chans} channels: {secs} s of input in {dt:.2} s = {:.0} % of one core",
        100.0 * dt / secs
    );
}
