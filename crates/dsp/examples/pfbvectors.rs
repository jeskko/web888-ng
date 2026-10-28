// SPDX-License-Identifier: GPL-3.0-or-later
//! Bit-exact test vectors for the channelizer's Verilog
//! (`hw/docs/channelizer.md` §3.6), from the fixed-point model with the
//! chosen widths and the radix-2² arithmetic:
//!
//!   cargo run --release -p web888-dsp --example pfbvectors -- OUTDIR [FRAMES]
//!
//! Files (`$readmemh` format, one value per line, two's complement; a
//! complex value is {re, im}, each `bits` wide):
//! * `meta.txt`: sizes, widths, shifts, the scale, what each file holds
//! * `coefs.hex`: h[r + t·M] (coef_bits)
//! * `tw.hex`: W_n^k, k < n; `split_tw.hex`: W_M^k, k <= n (twiddle_bits)
//! * `adc.hex`: the 16-bit input samples
//! * `packed.hex`, `fft.hex`, `bins.hex`: per frame, the FFT input (n),
//!   its bit-reversed output (n) and the M/2 + 1 bins (data_bits)
//!
//! The input: ADC noise of 3 LSB rms, a tone at -1 dBFS and seven weaker
//! ones, so every stage sees large and small values.

use std::fmt::Write as _;
use std::path::PathBuf;
use web888_dsp::pfb::PfbSpec;
use web888_dsp::pfbfx::{FixedSpec, PfbFixed};

fn hex(v: i64, bits: u32) -> String {
    let mask = (1u128 << bits) - 1;
    let digits = bits.div_ceil(4) as usize;
    format!("{:0digits$x}", (v as i128 as u128) & mask)
}

fn cx(v: (i64, i64), bits: u32) -> String {
    let mask = (1u128 << bits) - 1;
    let w = (((v.0 as i128 as u128) & mask) << bits) | ((v.1 as i128 as u128) & mask);
    let digits = (2 * bits).div_ceil(4) as usize;
    format!("{w:0digits$x}")
}

fn adc(n: usize, seed: u64) -> Vec<i16> {
    let tones: Vec<(f64, f64)> = std::iter::once((7.0745e6, -1.0))
        .chain((0..7).map(|k| (1.9e6 + 7.3e6 * k as f64, -30.0 - 6.0 * k as f64)))
        .collect();
    let mut s = seed;
    (0..n)
        .map(|i| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            let u = (s >> 11) as f64 / (1u64 << 53) as f64 - 0.5;
            let t = i as f64 / 122.88e6;
            let v: f64 = tones
                .iter()
                .map(|&(f, db)| {
                    10f64.powf(db / 20.0) * 32767.0 * (2.0 * std::f64::consts::PI * f * t).cos()
                })
                .sum::<f64>()
                * 0.5 // the sum of all eight stays inside full scale
                + 3.0 * 3.4641 * u;
            v.round().clamp(-32768.0, 32767.0) as i16
        })
        .collect()
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let out = PathBuf::from(args.get(1).expect("usage: pfbvectors OUTDIR [FRAMES]"));
    let frames: usize = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(6);
    std::fs::create_dir_all(&out)?;
    let spec = PfbSpec::web888();
    let fx = FixedSpec::recommended(&spec);
    let mut p = PfbFixed::new(spec, fx);
    let fx = *p.fixed(); // with the branch shift it chose
    p.record(true);
    let m = spec.branches;
    let n = m / 2;
    // the first frame needs M·T samples, each next one M/2 more
    let x = adc(m * spec.taps + (frames - 1) * n, 1);

    let (mut packed, mut fft, mut bins) = (String::new(), String::new(), String::new());
    // n samples at a time: at most one frame per push
    for chunk in x.chunks(n) {
        let mut got = false;
        p.push(chunk, |_| got = true);
        if !got {
            continue;
        }
        let t = p.taps();
        for v in &t.packed {
            writeln!(packed, "{}", cx(*v, fx.data_bits)).unwrap();
        }
        for v in &t.fft {
            writeln!(fft, "{}", cx(*v, fx.data_bits)).unwrap();
        }
        for v in &t.bins {
            writeln!(bins, "{}", cx(*v, fx.data_bits)).unwrap();
        }
    }
    let lines = |s: &str| s.lines().count();
    assert_eq!(lines(&bins), frames * spec.bins(), "frames written");
    std::fs::write(out.join("packed.hex"), packed)?;
    std::fs::write(out.join("fft.hex"), fft)?;
    std::fs::write(out.join("bins.hex"), bins)?;
    let adc_hex: String = x.iter().map(|&v| hex(v as i64, 16) + "\n").collect();
    std::fs::write(out.join("adc.hex"), adc_hex)?;
    let coefs: String = p
        .coefs()
        .iter()
        .map(|&v| hex(v, fx.coef_bits) + "\n")
        .collect();
    std::fs::write(out.join("coefs.hex"), coefs)?;
    let (tw, stw) = p.twiddles();
    let f =
        |v: &[(i64, i64)]| -> String { v.iter().map(|&c| cx(c, fx.twiddle_bits) + "\n").collect() };
    std::fs::write(out.join("tw.hex"), f(&tw))?;
    std::fs::write(out.join("split_tw.hex"), f(&stw))?;
    let meta = format!(
        "branches M = {m}\ntaps T = {}\nfft points n = {n}\nframes = {frames}\n\
         adc samples = {}\ncoef_bits = {}\ncoef_shift = {}\ndata_bits = {}\n\
         twiddle_bits = {}\nbranch_shift = {}\nstage_shifts = {:#013b} (bit s: halve after stage s)\n\
         arch = {:?}\nsaturations = {}\n\
         frame f uses adc[f·n .. f·n + M·T]; packed/fft: n lines per frame, bins: n + 1\n\
         complex values: {{re, im}}, each data_bits (twiddles: twiddle_bits) wide\n",
        spec.taps,
        x.len(),
        fx.coef_bits,
        p.coef_shift(),
        fx.data_bits,
        fx.twiddle_bits,
        fx.branch_shift,
        fx.stage_shifts,
        fx.arch,
        p.saturations,
    );
    std::fs::write(out.join("meta.txt"), meta)?;
    println!("{} frames into {}", frames, out.display());
    Ok(())
}
