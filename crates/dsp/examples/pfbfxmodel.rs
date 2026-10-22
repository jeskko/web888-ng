// SPDX-License-Identifier: GPL-3.0-or-later
//! Channelizer fixed-point widths (hw/docs/channelizer.md §5 step 2): the
//! integer model against the float one on the same 16-bit samples.
//! For each width and FFT scaling: how far the fixed-point error stays
//! below the ADC noise in a bin, and whether anything saturates with a
//! signal near full scale.
//!   cargo run --release -p web888-dsp --example pfbfxmodel [--all] [--coef N]
use web888_dsp::pfb::{Pfb, PfbSpec};
use web888_dsp::pfbfx::{FixedSpec, PfbFixed};

const FS: f64 = 122.88e6;

/// Tones (Hz, dBFS) plus ADC noise (3 LSB rms), as 16-bit samples.
fn adc(n: usize, tones: &[(f64, f64)], seed: u64) -> Vec<i16> {
    let mut s = seed;
    (0..n)
        .map(|i| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            let u = (s >> 11) as f64 / (1u64 << 53) as f64 - 0.5;
            let mut v = 3.0 * 3.4641 * u;
            for (k, &(f, db)) in tones.iter().enumerate() {
                let a = 10f64.powf(db / 20.0) * 32767.0;
                v += a * (2.0 * std::f64::consts::PI * f / FS * i as f64 + k as f64).cos();
            }
            v.round().clamp(-32768.0, 32767.0) as i16
        })
        .collect()
}

/// (dB the error stays below the noise in bins away from tones, saturations)
fn run(spec: PfbSpec, fx: FixedSpec, x: &[i16], tones: &[(f64, f64)]) -> (f64, u64) {
    let xf: Vec<f32> = x.iter().map(|&v| v as f32 / 32768.0).collect();
    let mut fl = Pfb::new(spec);
    let mut fi = PfbFixed::new(spec, fx);
    let (mut a, mut b) = (Vec::new(), Vec::new());
    fl.push(&xf, |f| a.push(f.to_vec()));
    fi.push(x, |f| b.push(f.to_vec()));
    let near: Vec<usize> = tones.iter().map(|t| spec.bin_of(t.0)).collect();
    let (mut noise, mut err) = (0f64, 0f64);
    for (fa, fb) in a.iter().zip(&b) {
        for k in 1..spec.bins() - 1 {
            if near.iter().any(|&t| t.abs_diff(k) <= 3) {
                continue;
            }
            noise += fa[k].norm_sqr() as f64;
            err += (fb[k] - fa[k]).norm_sqr() as f64;
        }
    }
    (10.0 * (noise / err.max(1e-300)).log10(), fi.saturations)
}

fn main() {
    let spec = PfbSpec::web888();
    let n = 2048 * 80;
    let one = [(7.074e6, -1.0)];
    let x1 = adc(n, &one, 1);
    // eight tones summing to about full scale at their peaks
    let many: Vec<(f64, f64)> = (0..8).map(|k| (3.5e6 + 2.31e6 * k as f64, -19.0)).collect();
    let x8 = adc(n, &many, 2);
    let stages = 11;
    println!(
        "data twid  halvings          tone -1 dBFS: err below noise, sat   8 tones: below, sat"
    );
    let wide = std::env::args().any(|a| a == "--all");
    // --coef N: coefficient width (18 = DSP48 B port; up to 25 with the
    // ports swapped: ADC data on B, coefficients on A)
    let args: Vec<String> = std::env::args().collect();
    let coef: u32 = args
        .iter()
        .position(|a| a == "--coef")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(18);
    println!("coefficients: {coef} bits");
    let datas: &[u32] = if wide { &[20, 22, 24, 25] } else { &[24, 25] };
    for &data in datas {
        for tw in [16, 18] {
            for halv in [5, 6, 7, 8, 9, 10, 11] {
                // halvings in the first stages, the last ones, or split
                for (place, mask) in [
                    ("last", ((1u32 << halv) - 1) << (stages - halv)),
                    ("first", (1u32 << halv) - 1),
                    (
                        "split",
                        ((1u32 << (halv / 2)) - 1)
                            | (((1u32 << (halv - halv / 2)) - 1) << (stages - (halv - halv / 2))),
                    ),
                ] {
                    if halv == 11 && place != "last" {
                        continue;
                    }
                    let fx = FixedSpec {
                        coef_bits: coef,
                        data_bits: data,
                        twiddle_bits: tw,
                        stage_shifts: mask,
                        ..FixedSpec::default_for(&spec)
                    };
                    let (e1, s1) = run(spec, fx, &x1, &one);
                    let (e8, s8) = run(spec, fx, &x8, &many);
                    println!(
                        "{data:4} {tw:4}  {halv:2} ({place:5})          {e1:8.1} dB  {s1:8}            {e8:8.1} dB {s8:8}"
                    );
                }
            }
        }
    }
}
