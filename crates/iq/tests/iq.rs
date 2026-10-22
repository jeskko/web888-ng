// SPDX-License-Identifier: GPL-3.0-or-later
//! IqRadio end to end on synthetic wideband input: 7.68 MS/s around
//! 10.5 MHz (channels within +-3.46 MHz: 7.04..13.96 MHz), read directly
//! (no pacing).

use num_complex::Complex32;
use std::io;
use web888_iq::source::{FileSource, Format, IqSource};
use web888_iq::{IqConfig, IqRadio};
use web888_radio::*;

const FS: f64 = 7.68e6;
const FC: f64 = 10.5e6;

/// Complex tones (absolute frequency, amplitude) as seen by an SDR tuned
/// to FC, endless.
struct Tones {
    tones: Vec<(f64, f64)>,
    n: u64,
}

impl IqSource for Tones {
    fn read(&mut self, buf: &mut [Complex32]) -> io::Result<usize> {
        for o in buf.iter_mut() {
            let t = self.n as f64 / FS;
            let mut z = (0.0, 0.0);
            for &(f, a) in &self.tones {
                let p = 2.0 * std::f64::consts::PI * (f - FC) * t;
                z.0 += a * p.cos();
                z.1 += a * p.sin();
            }
            *o = Complex32::new(z.0 as f32, z.1 as f32);
            self.n += 1;
        }
        Ok(buf.len())
    }
}

fn radio(tones: &[(f64, f64)]) -> IqRadio {
    let mut cfg = IqConfig::new(FC, FS);
    cfg.dc_block = false;
    IqRadio::direct(
        cfg,
        Box::new(Tones {
            tones: tones.to_vec(),
            n: 0,
        }),
    )
    .unwrap()
}

/// (mean |z|, mean frequency in Hz) of channel `c` over frames `skip..`.
fn measure(frames: &[Iq32], n: usize, c: usize, skip: usize, fs: f64) -> (f64, f64) {
    let z: Vec<(f64, f64)> = frames
        .chunks(n)
        .skip(skip)
        .map(|f| (f[c].i as f64, f[c].q as f64))
        .collect();
    let amp = z.iter().map(|(i, q)| (i * i + q * q).sqrt()).sum::<f64>() / z.len() as f64;
    let (mut re, mut im) = (0.0, 0.0);
    for w in z.windows(2) {
        re += w[1].0 * w[0].0 + w[1].1 * w[0].1;
        im += w[1].1 * w[0].0 - w[1].0 * w[0].1;
    }
    (amp, im.atan2(re) * fs / (2.0 * std::f64::consts::PI))
}

#[test]
fn channels_see_their_tones_at_hardware_levels() {
    let mut r = radio(&[(7_075_000.0, 0.1), (13_900_500.0, 0.01)]);
    r.set_rx_rate(10).unwrap(); // 48 kS/s
    r.tune_rx(0, 7_074_000.0).unwrap();
    r.tune_rx(1, 13_900_000.0).unwrap();
    // 7.68 MHz above the 7.075 MHz tone: beyond the band, the offset wraps
    // onto the tone (what an SDR's filter lets fold in)
    r.tune_rx(2, 7_074_000.0 + FS).unwrap();
    let n = r.caps().rx_channels;
    let mut buf = vec![Iq32::default(); 4800 * n];
    assert_eq!(r.read_rx(&mut buf).unwrap(), 4800);

    let (a0, f0) = measure(&buf, n, 0, 100, 48_000.0);
    assert!((a0 / (0.1 * RX_SINE_FULL_SCALE) - 1.0).abs() < 0.01, "{a0}");
    assert!((f0 - 1000.0).abs() < 1.0, "{f0}");
    let (a1, f1) = measure(&buf, n, 1, 100, 48_000.0);
    assert!(
        (a1 / (0.01 * RX_SINE_FULL_SCALE) - 1.0).abs() < 0.01,
        "{a1}"
    );
    assert!((f1 - 500.0).abs() < 1.0, "{f1}");
    let (a2, f2) = measure(&buf, n, 2, 100, 48_000.0);
    assert!((a2 / (0.1 * RX_SINE_FULL_SCALE) - 1.0).abs() < 0.01, "{a2}");
    assert!((f2 - 1000.0).abs() < 1.0, "{f2}");

    // a channel tuned mid-stream joins the same frames
    r.tune_rx(3, 7_074_000.0).unwrap();
    assert_eq!(r.read_rx(&mut buf).unwrap(), 4800);
    let (a3, f3) = measure(&buf, n, 3, 100, 48_000.0);
    assert!((a3 / (0.1 * RX_SINE_FULL_SCALE) - 1.0).abs() < 0.01, "{a3}");
    assert!((f3 - 1000.0).abs() < 1.0, "{f3}");

    // attenuator: 10 dB
    r.front_end(FrontEnd {
        atten_db: 10.0,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(r.read_rx(&mut buf).unwrap(), 4800);
    let (a0, _) = measure(&buf, n, 0, 100, 48_000.0);
    let db = 20.0 * (a0 / (0.1 * RX_SINE_FULL_SCALE)).log10();
    assert!((db + 10.0).abs() < 0.1, "{db}");
}

#[test]
fn waterfall_one_shot_and_limits() {
    let mut r = radio(&[(7_100_000.0, 0.5)]);
    // narrower than the input: fine; one block, then nothing
    r.start_wf(0, 7_074_000.0, 64, WfMode::OneShot).unwrap();
    let mut buf = vec![Iq16::default(); 4096];
    let mut total = 0;
    for _ in 0..10 {
        total += r.read_wf(0, &mut buf).unwrap();
    }
    assert_eq!(total, WF_BLOCK);
    // 7.1 MHz is 26 kHz above the NCO at 1.92 MS/s: level and frequency
    let last = &buf[..];
    let amp = last
        .iter()
        .map(|s| ((s.i as f64).powi(2) + (s.q as f64).powi(2)).sqrt())
        .sum::<f64>()
        / last.len() as f64;
    assert!(
        (amp / (0.5 * WF_SINE_FULL_SCALE) - 1.0).abs() < 0.02,
        "{amp}"
    );
    // wider than the input: refused
    assert!(r.start_wf(1, 7_074_000.0, 8, WfMode::Continuous).is_err());
    // continuous keeps coming
    r.start_wf(1, 7_074_000.0, 1024, WfMode::Continuous)
        .unwrap();
    let got: usize = (0..3).map(|_| r.read_wf(1, &mut buf).unwrap()).sum();
    assert_eq!(got, 3 * 4096);
}

#[test]
fn clipping_counts_as_adc_overflow() {
    let mut r = radio(&[(7_000_000.0, 1.2)]);
    let n = r.caps().rx_channels;
    let mut buf = vec![Iq32::default(); 100 * n];
    r.read_rx(&mut buf).unwrap();
    assert!(r.hw_stats().unwrap().unwrap().adc_overflows > 0);
    let mut r = radio(&[(7_000_000.0, 0.5)]);
    r.read_rx(&mut buf).unwrap();
    assert_eq!(r.hw_stats().unwrap().unwrap().adc_overflows, 0);
}

#[test]
fn hackrf_cs8_file() {
    // what `hackrf_transfer -r x.cs8 -f 10500000 -s 7680000` writes
    let path = std::env::temp_dir().join(format!("w888-iq-{}.cs8", std::process::id()));
    let mut t = Tones {
        tones: vec![(10_000_000.0, 0.5)],
        n: 0,
    };
    let mut s = vec![Complex32::default(); 160 * 3000];
    t.read(&mut s).unwrap();
    let bytes: Vec<u8> = s
        .iter()
        .flat_map(|z| {
            [
                (z.re * 127.0).round() as i8 as u8,
                (z.im * 127.0).round() as i8 as u8,
            ]
        })
        .collect();
    std::fs::write(&path, bytes).unwrap();

    let src = FileSource::open(&path, Format::from_path(&path).unwrap(), false).unwrap();
    let mut cfg = IqConfig::new(FC, FS);
    cfg.dc_block = false;
    let mut r = IqRadio::direct(cfg, Box::new(src)).unwrap();
    r.set_rx_rate(10).unwrap();
    r.tune_rx(0, 9_998_000.0).unwrap();
    let n = r.caps().rx_channels;
    let mut buf = vec![Iq32::default(); 3000 * n];
    let got = r.read_rx(&mut buf).unwrap();
    assert_eq!(got, 3000);
    let (a, f) = measure(&buf[..got * n], n, 0, 100, 48_000.0);
    // 8-bit input: level within 0.05 dB of the FPGA chain response (CIC
    // droop at 2 kHz: -0.12 dB), frequency exact
    let want = 0.5 * 127.0 / 128.0 * RX_SINE_FULL_SCALE * rx_chain_mag(2000.0, ADC_HZ_HF, 10);
    assert!((20.0 * (a / want).log10()).abs() < 0.05, "{a} vs {want}");
    assert!((f - 2000.0).abs() < 1.0, "{f}");
    // end of file: no more frames
    assert_eq!(r.read_rx(&mut buf).unwrap(), 0);
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn only_tuned_channels_are_computed() {
    let mut r = radio(&[]);
    // 0 Hz (an untuned channel's word) is within the wrapped range of an
    // 8.6 MHz input, but untuned channels cost nothing
    assert_eq!(r.active_rx(), 0);
    r.set_rx_rate(10).unwrap();
    assert_eq!(r.active_rx(), 0);
    r.tune_rx(0, 7.074e6).unwrap();
    r.tune_rx(3, 10.1e6).unwrap();
    assert_eq!(r.active_rx(), 2);
    r.idle_rx(0).unwrap();
    assert_eq!(r.active_rx(), 1);
    // idled channels read zeros
    let n = r.caps().rx_channels;
    let mut buf = vec![Iq32::default(); 480 * n];
    let mut got = 0;
    while got == 0 {
        got = r.read_rx(&mut buf).unwrap();
    }
    assert!(buf[..got * n].chunks(n).all(|f| f[0] == Iq32::default()));
    r.set_rx_rate(10).unwrap();
    assert_eq!(r.active_rx(), 1, "a rate change keeps the tuned one");
}
