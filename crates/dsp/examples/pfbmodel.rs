// SPDX-License-Identifier: GPL-3.0-or-later
//! Channelizer model (hw/docs/channelizer.md): alias rejection and ripple
//! against taps per branch, to size the prototype filter (BRAM).
//!   cargo run --release -p web888-dsp --example pfbmodel
use num_complex::Complex64;
use web888_dsp::pfb::{Pfb, PfbSpec};

/// Amplitude at `off` Hz (folded to the bin rate) in bin `k`, for a tone
/// of amplitude `a` at `k * bin + off`.
fn level(spec: PfbSpec, k: usize, off: f64, a: f64) -> f64 {
    let mut pfb = Pfb::new(spec);
    let f = k as f64 * spec.bin_hz() + off;
    let w = 2.0 * std::f64::consts::PI * f / spec.fs;
    let n = 2048 * (spec.taps * 2 + 60);
    let x: Vec<f32> = (0..n).map(|i| (a * (w * i as f64).cos()) as f32).collect();
    let mut s = Vec::new();
    pfb.push(&x, |fr| s.push(fr[k]));
    let s = &s[spec.taps * 2 + 2..];
    let r = spec.out_rate();
    let alias = off - r * (off / r).round();
    let mut acc = Complex64::new(0.0, 0.0);
    for (i, z) in s.iter().enumerate() {
        let p = Complex64::from_polar(1.0, -2.0 * std::f64::consts::PI * alias / r * i as f64);
        acc += Complex64::new(z.re as f64, z.im as f64) * p;
    }
    acc.norm() / s.len() as f64
}

fn main() {
    let base = PfbSpec::web888();
    println!(
        "M {} ({:.0} Hz bins, {:.0} S/s each), pass ±{:.0} Hz, stop ±{:.0} Hz",
        base.branches,
        base.bin_hz(),
        base.out_rate(),
        base.pass_hz,
        base.stop_hz
    );
    println!("  T  design dB   ripple dB   worst alias dB (offsets folding into the passband)");
    for taps in [8, 9, 10, 11, 12, 13] {
        for atten in [90.0, 100.0, 110.0] {
            let spec = PfbSpec {
                taps,
                atten_db: atten,
                ..base
            };
            let k = 300;
            let ripple = [-21e3, -14e3, -7e3, 0.0, 7e3, 14e3, 21e3]
                .iter()
                .map(|&o| (20.0 * (level(spec, k, o, 0.5) / 0.5).log10()).abs())
                .fold(0.0f64, f64::max);
            // stopband offsets whose alias lands within ±pass of the centre
            let mut worst = f64::MIN;
            let mut off = spec.stop_hz;
            while off <= 150e3 {
                for o in [off, -off] {
                    let r = spec.out_rate();
                    let alias = o - r * (o / r).round();
                    if alias.abs() <= spec.pass_hz {
                        let db = 20.0 * (level(spec, k, o, 0.5) / 0.5).max(1e-12).log10();
                        worst = worst.max(db);
                    }
                }
                off += 1500.0;
            }
            println!("{taps:3}  {atten:9.0}   {ripple:9.4}   {worst:8.1}");
        }
    }
}
