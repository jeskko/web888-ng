// SPDX-License-Identifier: GPL-3.0-or-later
//! One recording through an RX channel and through the bins (`BinRx`,
//! `doc/m6-channelizer.md` step 3): how alike the two channels are, as
//! the magnitude-squared coherence and the level ratio within ±3 kHz and
//! ±12 kHz of the dial (48 kS/s), after aligning them.
//!   cargo run --release -p web888-iq --example binscompare -- FILE.cs8 CENTRE_HZ [DIAL_HZ...]
use num_complex::Complex64;
use rustfft::FftPlanner;
use std::path::Path;
use web888_dsp::binrx::BinRx;
use web888_iq::source::{FileSource, Format};
use web888_iq::{IqConfig, IqRadio};
use web888_radio::{Iq32, Radio};

const SECS: f64 = 20.0;
const FS: f64 = 48e3;

fn collect(r: &mut dyn Radio, nch: usize, n: usize) -> Vec<Complex64> {
    let mut buf = vec![Iq32::default(); nch * 1024];
    let mut z = Vec::with_capacity(n);
    while z.len() < n {
        let k = r.read_rx(&mut buf).unwrap();
        z.extend((0..k).map(|f| Complex64::new(buf[f * nch].i as f64, buf[f * nch].q as f64)));
    }
    z.truncate(n);
    z
}

/// The lag (samples) of `b` against `a` with the largest |correlation|.
fn lag(a: &[Complex64], b: &[Complex64], max: usize) -> isize {
    let n = a.len() - 2 * max;
    (-(max as isize)..=max as isize)
        .map(|l| {
            let c: Complex64 = (0..n)
                .map(|i| a[i + max] * b[(i as isize + max as isize + l) as usize].conj())
                .sum();
            (l, c.norm())
        })
        .max_by(|x, y| x.1.total_cmp(&y.1))
        .unwrap()
        .0
}

/// Welch: per frequency bin Sxx, Syy, Sxy (segments of `len`, Hann, half overlap).
fn welch(a: &[Complex64], b: &[Complex64], len: usize) -> (Vec<f64>, Vec<f64>, Vec<Complex64>) {
    let fft = FftPlanner::new().plan_fft_forward(len);
    let w: Vec<f64> = (0..len)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / len as f64).cos())
        .collect();
    let (mut sxx, mut syy, mut sxy) = (
        vec![0.0; len],
        vec![0.0; len],
        vec![Complex64::default(); len],
    );
    let mut s = 0;
    while s + len <= a.len().min(b.len()) {
        let mut x: Vec<Complex64> = (0..len).map(|i| a[s + i] * w[i]).collect();
        let mut y: Vec<Complex64> = (0..len).map(|i| b[s + i] * w[i]).collect();
        fft.process(&mut x);
        fft.process(&mut y);
        for k in 0..len {
            sxx[k] += x[k].norm_sqr();
            syy[k] += y[k].norm_sqr();
            sxy[k] += x[k] * y[k].conj();
        }
        s += len / 2;
    }
    (sxx, syy, sxy)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let file = args
        .first()
        .expect("usage: FILE.cs8 CENTRE_HZ [DIAL_HZ...]")
        .clone();
    let centre: f64 = args.get(1).and_then(|s| s.parse().ok()).expect("CENTRE_HZ");
    let dials: Vec<f64> = if args.len() > 2 {
        args[2..].iter().map(|s| s.parse().unwrap()).collect()
    } else {
        vec![7.074e6, 10.136e6]
    };
    let open = || {
        let p = Path::new(&file);
        Box::new(FileSource::open(p, Format::from_path(p).unwrap(), false).unwrap())
    };
    let n = (SECS * FS) as usize;
    for dial in dials {
        let mut a = IqRadio::direct(IqConfig::new(centre, 7.68e6), open()).unwrap();
        a.set_rx_rate(10).unwrap();
        a.tune_rx(0, dial).unwrap();
        let na = a.caps().rx_channels;
        let za = collect(&mut a, na, n);
        let mut cfg = IqConfig::new(centre, 7.68e6);
        cfg.bins = true;
        let mut b = BinRx::new(IqRadio::direct(cfg, open()).unwrap(), 1).unwrap();
        b.tune_rx(0, dial).unwrap();
        let zb = collect(&mut b, 1, n);

        let l = lag(&za[..n / 4], &zb[..n / 4], 1500);
        let skip = 4800; // both filled
        let (a2, b2): (Vec<_>, Vec<_>) = if l >= 0 {
            (
                za[skip..n - l as usize].to_vec(),
                zb[skip + l as usize..].to_vec(),
            )
        } else {
            (
                za[skip + (-l) as usize..].to_vec(),
                zb[skip..n - (-l) as usize].to_vec(),
            )
        };
        let len = 4096;
        let (sxx, syy, sxy) = welch(&a2, &b2, len);
        for band in [3e3, 12e3] {
            let (mut p, mut q) = (0.0, 0.0);
            let (mut msc_w, mut wsum) = (0.0, 0.0);
            for k in 0..len {
                let f = if k < len / 2 {
                    k as f64
                } else {
                    k as f64 - len as f64
                } * FS
                    / len as f64;
                if f.abs() > band {
                    continue;
                }
                p += sxx[k];
                q += syy[k];
                let msc = sxy[k].norm_sqr() / (sxx[k] * syy[k]);
                msc_w += msc * sxx[k];
                wsum += sxx[k];
            }
            let msc = msc_w / wsum;
            println!(
                "{:.3} MHz ±{:.0} kHz: lag {l}, level bins/RX {:+.2} dB, coherence {:.5} ({:.1} dB residual)",
                dial / 1e6,
                band / 1e3,
                10.0 * (q / p).log10(),
                msc,
                10.0 * (1.0 - msc).max(1e-12).log10()
            );
        }
    }
}
