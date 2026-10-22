// SPDX-License-Identifier: GPL-3.0-or-later
//! Listen to a recording through the channelizer and through today's
//! chain (hw/docs/channelizer.md §5): writes `<out>-tuned.wav` (the iq
//! backend's DDC: NCO, CIC N=3 to 480 kS/s, CIC N=5 R=10, audio chain at
//! 48 kS/s) and `<out>-pfb.wav` (the complex channelizer: 30 kHz bins at
//! 60 kS/s, shift, audio chain), both 12 kHz mono with AGC.
//!   pfbwav REC.cs8 --center HZ --scan
//!   pfbwav REC.cs8 --center HZ --dial HZ --mode usb|lsb|cw|am [--secs S] [--out PREFIX]
//!     [--gain DB]  (fixed gain instead of AGC, for comparing the paths)
use num_complex::Complex32;
use rustfft::FftPlanner;
use std::io::Write;
use std::path::Path;
use web888_dsp::C32;
use web888_dsp::audio::{AudioChain, AudioSpec, Mode};
use web888_dsp::chan::Droop;
use web888_dsp::pfb::{PfbChannel, PfbComplex, PfbSpec};
use web888_iq::ddc::Ddc;
use web888_iq::source::{FileSource, Format, IqSource};

const FS: f64 = 7.68e6;

fn arg(a: &[String], k: &str) -> Option<String> {
    a.iter()
        .position(|x| x == k)
        .and_then(|i| a.get(i + 1))
        .cloned()
}

fn write_wav(path: &str, pcm: &[f32]) -> std::io::Result<()> {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    let n = (pcm.len() * 2) as u32;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + n).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?; // PCM
    f.write_all(&1u16.to_le_bytes())?; // mono
    f.write_all(&12_000u32.to_le_bytes())?;
    f.write_all(&24_000u32.to_le_bytes())?;
    f.write_all(&2u16.to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&n.to_le_bytes())?;
    for &v in pcm {
        f.write_all(&((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())?;
    }
    Ok(())
}

/// Strongest carriers (averaged 8192-point spectrum of the first seconds).
fn scan(src: &mut dyn IqSource, center: f64, secs: f64) {
    let n = 8192;
    let fft = FftPlanner::<f32>::new().plan_fft_forward(n);
    let mut acc = vec![0f64; n];
    let mut buf = vec![Complex32::default(); n];
    for _ in 0..(secs * FS) as usize / n {
        if src.read(&mut buf).unwrap_or(0) < n {
            break;
        }
        let mut b: Vec<C32> = buf
            .iter()
            .enumerate()
            .map(|(k, z)| {
                z * (0.5 - 0.5 * (2.0 * std::f32::consts::PI * k as f32 / n as f32).cos())
            })
            .collect();
        fft.process(&mut b);
        for (a, z) in acc.iter_mut().zip(&b) {
            *a += z.norm_sqr() as f64;
        }
    }
    let bin = FS / n as f64;
    let mut peaks: Vec<(f64, f64)> = (0..n)
        .filter(|&k| {
            let (l, r) = (acc[(k + n - 1) % n], acc[(k + 1) % n]);
            acc[k] > l && acc[k] > r
        })
        .map(|k| {
            let off = if k < n / 2 {
                k as f64
            } else {
                k as f64 - n as f64
            } * bin;
            (center + off, 10.0 * acc[k].log10())
        })
        .filter(|(f, _)| (f - center).abs() < 0.45 * FS)
        .collect();
    peaks.sort_by(|a, b| b.1.total_cmp(&a.1));
    println!("strongest carriers (kHz, dB rel):");
    let top = peaks[0].1;
    for (f, p) in peaks.iter().take(25) {
        println!("  {:10.1}  {:6.1}", f / 1e3, p - top);
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let path = a
        .get(1)
        .expect("usage: pfbwav REC.cs8 --center HZ (--scan | --dial HZ --mode M)");
    let center: f64 = arg(&a, "--center")
        .and_then(|v| v.parse().ok())
        .expect("--center");
    let fmt = Format::from_path(Path::new(path)).expect(".cs8/.cs16/.cf32");
    let mut src = FileSource::open(Path::new(path), fmt, false).expect("open");
    if a.iter().any(|x| x == "--scan") {
        scan(&mut src, center, 3.0);
        return;
    }
    let dial: f64 = arg(&a, "--dial")
        .and_then(|v| v.parse().ok())
        .expect("--dial");
    let mode = match arg(&a, "--mode").as_deref().unwrap_or("usb") {
        "lsb" => Mode::Lsb,
        "cw" => Mode::Cw,
        "am" => Mode::Am,
        _ => Mode::Usb,
    };
    let secs: f64 = arg(&a, "--secs")
        .and_then(|v| v.parse().ok())
        .unwrap_or(20.0);
    let out =
        arg(&a, "--out").unwrap_or_else(|| format!("{:.0}-{:?}", dial / 1e3, mode).to_lowercase());

    let off = dial - center;
    let gain: Option<f32> = arg(&a, "--gain").and_then(|v| v.parse().ok());
    let audio = || {
        let mut s = AudioSpec::new(mode);
        if let Some(g) = gain {
            s.agc = None;
            s.gain_db = g;
        }
        s
    };
    let mut ddc = Ddc::rx(off, FS, 16, 10);
    let mut tuned = AudioChain::new(
        audio(),
        48_000.0,
        Droop::Rx {
            adc_hz: 122.88e6,
            r2: 10,
        },
    );
    let spec = PfbSpec {
        fs: FS,
        branches: 256,
        ..PfbSpec::web888()
    };
    let mut pfb = PfbComplex::new(spec);
    let (k, rest) = pfb.bin_of(off);
    let mut ch = PfbChannel::at(&spec, k, rest, audio());
    println!(
        "dial {:.3} kHz: bin {k} ({:+.1} kHz from its centre)",
        dial / 1e3,
        rest / 1e3
    );

    let (mut pa, mut pb) = (Vec::new(), Vec::new());
    let mut buf = vec![Complex32::default(); 76_800]; // 10 ms
    let (mut y, mut bins) = (Vec::new(), Vec::new());
    for _ in 0..(secs * 100.0) as usize {
        let got = src.read(&mut buf).unwrap_or(0);
        if got == 0 {
            break;
        }
        let x = &buf[..got];
        ddc.process(x, &mut y);
        // the DDC's units are the input's; the chain wants a full-scale
        // sine at 1, as rx_to_c32 gives (the AGC makes the level moot)
        tuned.process_c32(
            y.iter().map(|z| C32::new(z.re as f32, z.im as f32)),
            &mut pa,
        );
        bins.clear();
        pfb.push(x, |f| bins.push(f[k]));
        ch.process(&bins, &mut pb);
    }
    for (name, pcm) in [("tuned", &pa), ("pfb", &pb)] {
        let p = format!("{out}-{name}.wav");
        write_wav(&p, pcm).expect("write");
        let rms =
            (pcm.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / pcm.len().max(1) as f64).sqrt();
        println!("{p}: {:.1} s, rms {:.3}", pcm.len() as f64 / 12e3, rms);
    }
}
