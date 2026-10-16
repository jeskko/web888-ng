// SPDX-License-Identifier: GPL-3.0-or-later
use num_complex::Complex64;
use web888_dsp::chan::{ChannelSpec, Droop, design, response};
use web888_dsp::demod::{Am, ssb};
use web888_dsp::fir::{kaiser_len, lowpass};
use web888_dsp::ols::OverlapSave;
use web888_dsp::{C32, rx_to_c32};
use web888_radio::*;
use web888_sim::*;

fn db(x: f64) -> f64 {
    20.0 * x.log10()
}

fn c64(h: &[C32]) -> Vec<Complex64> {
    h.iter()
        .map(|c| Complex64::new(c.re as f64, c.im as f64))
        .collect()
}

/// Amplitude of a complex exponential at `f` in `z` (Blackman-Harris
/// windowed DFT, -92 dB sidelobes).
fn tone_amp(z: &[C32], f: f64, fs: f64) -> f64 {
    let n = z.len();
    let (a0, a1, a2, a3) = (0.35875, 0.48829, 0.14128, 0.01168);
    let mut acc = Complex64::new(0.0, 0.0);
    let mut wsum = 0.0;
    for (k, x) in z.iter().enumerate() {
        let t = 2.0 * std::f64::consts::PI * k as f64 / (n - 1) as f64;
        let w = a0 - a1 * t.cos() + a2 * (2.0 * t).cos() - a3 * (3.0 * t).cos();
        let ph = Complex64::from_polar(1.0, -2.0 * std::f64::consts::PI * f * k as f64 / fs);
        acc += Complex64::new(x.re as f64, x.im as f64) * ph * w;
        wsum += w;
    }
    acc.norm() / wsum
}

fn real_amp(x: &[f32], f: f64, fs: f64) -> f64 {
    let z: Vec<C32> = x.iter().map(|&v| C32::new(v, 0.0)).collect();
    2.0 * tone_amp(&z, f, fs)
}

#[test]
fn kaiser_lowpass_meets_spec() {
    let n = kaiser_len(80.0, 0.05);
    let h = lowpass(n, 0.1, 80.0);
    for i in 0..=75 {
        let f = i as f64 / 1000.0;
        assert!(db(response(&h, f, 1.0)).abs() < 0.01, "passband {f}");
    }
    for i in 125..=500 {
        let f = i as f64 / 1000.0;
        assert!(db(response(&h, f, 1.0)) < -79.0, "stopband {f}");
    }
}

#[test]
fn overlap_save_equals_convolution() {
    let taps: Vec<C32> = (0..37)
        .map(|k| C32::new((k as f32 * 0.37).sin(), (k as f32 * 0.11).cos()))
        .collect();
    let x: Vec<C32> = (0..3000)
        .map(|k| C32::new((k as f32 * 0.013).sin(), (k as f32 * 0.071).cos()))
        .collect();
    let mut ols = OverlapSave::new(&taps);
    let mut y = Vec::new();
    ols.process(&x[..1234], &mut y);
    ols.process(&x[1234..], &mut y);
    assert!(y.len() > 2000);
    for (k, yk) in y.iter().enumerate() {
        let mut d = C32::new(0.0, 0.0);
        for (j, h) in taps.iter().enumerate() {
            if k >= j {
                d += h * x[k - j];
            }
        }
        assert!((yk - d).norm() < 1e-4, "sample {k}: {yk} vs {d}");
    }
}

#[test]
fn usb_channel_flat_with_droop() {
    let r2 = 20;
    let fs = rx_rate(ADC_HZ_HF, r2);
    let droop = Droop::Rx {
        adc_hz: ADC_HZ_HF,
        r2,
    };
    let h = c64(&design(ChannelSpec::usb(300.0, 2700.0), fs, droop));
    for f in (300..=2700).step_by(50) {
        let g = db(response(&h, f as f64, fs) * droop.mag(f as f64));
        assert!(g.abs() < 0.05, "passband {f}: {g:.3} dB");
    }
    for f in (-12000..=-100).step_by(50).chain((2900..12000).step_by(50)) {
        let g = db(response(&h, f as f64, fs) * droop.mag(f as f64));
        assert!(g < -78.0, "stopband {f}: {g:.1} dB");
    }
}

#[test]
fn wide_channel_compensates_cic_droop() {
    // +-9.6 kHz at 24 kS/s: the CIC alone droops ~12 dB at the edge
    let r2 = 20;
    let fs = rx_rate(ADC_HZ_HF, r2);
    let droop = Droop::Rx {
        adc_hz: ADC_HZ_HF,
        r2,
    };
    assert!(db(droop.mag(9600.0)) < -10.0);
    let spec = ChannelSpec {
        lo_hz: -9600.0,
        hi_hz: 9600.0,
        transition_hz: 500.0,
        atten_db: 70.0,
    };
    let h = c64(&design(spec, fs, droop));
    for f in (-9600..=9600).step_by(100) {
        let g = db(response(&h, f as f64, fs) * droop.mag(f as f64));
        assert!(g.abs() < 0.1, "passband {f}: {g:.3} dB");
    }
    for f in (10100..=12000).step_by(50) {
        for s in [-1.0, 1.0] {
            let g = db(response(&h, s * f as f64, fs) * droop.mag(f as f64));
            assert!(g < -68.0, "stopband {}: {g:.1} dB", s * f as f64);
        }
    }
}

/// M0 acceptance: IQ from the simulator through the droop-compensated
/// channel filter.
#[test]
fn sim_through_channel_filter() {
    let f0 = 7_074_000.0;
    let scene = Scene::new()
        .tone(f0 + 1500.0, -30.0) // wanted, USB
        .tone(f0 - 1500.0, -10.0) // opposite sideband
        .tone(f0 + 5000.0, -10.0) // adjacent
        .adc_noise();
    let mut r = SimRadio::new(SimConfig::hf(scene, Pacing::Free));
    let r2 = 20;
    r.set_rx_rate(r2).unwrap();
    r.tune_rx(5, f0).unwrap();
    let fs = rx_rate(ADC_HZ_HF, r2);
    let n = r.caps().rx_channels;

    let taps = design(
        ChannelSpec::usb(300.0, 2700.0),
        fs,
        Droop::Rx {
            adc_hz: ADC_HZ_HF,
            r2,
        },
    );
    let mut ols = OverlapSave::new(&taps);
    let mut buf = vec![Iq32::default(); 4096 * n];
    let mut y = Vec::new();
    for _ in 0..6 {
        let got = r.read_rx(&mut buf).unwrap();
        let ch: Vec<C32> = (0..got).map(|f| rx_to_c32(buf[f * n + 5])).collect();
        ols.process(&ch, &mut y);
    }
    let y = &y[ols.delay() + taps.len()..];
    assert!(y.len() > 16_000);

    // wanted tone: exactly -30 dBFS (droop compensated) within 0.05 dB
    let want = db(tone_amp(y, 1500.0, fs));
    assert!((want + 30.0).abs() < 0.05, "wanted at {want:.3} dBFS");
    // interferers 20 dB stronger at the input: >= 75 dB down at the output
    for f in [-1500.0, 5000.0] {
        let a = db(tone_amp(y, f, fs));
        assert!(a < -10.0 - 75.0, "interferer {f}: {a:.1} dBFS");
    }

    // USB demod: 1500 Hz audio at the same level (real sine, amplitude = |z|)
    let mut audio = Vec::new();
    ssb(y, &mut audio);
    let a = db(real_amp(&audio, 1500.0, fs));
    assert!((a + 30.0).abs() < 0.05, "audio {a:.3}");
}

#[test]
fn am_demod_depth() {
    let fc = 9_650_000.0;
    let scene = Scene::new().am(fc, -20.0, 1000.0, 0.5);
    let mut r = SimRadio::new(SimConfig::hf(scene, Pacing::Free));
    let r2 = 20;
    r.set_rx_rate(r2).unwrap();
    r.tune_rx(0, fc).unwrap();
    let fs = rx_rate(ADC_HZ_HF, r2);
    let n = r.caps().rx_channels;
    let taps = design(
        ChannelSpec::am(5000.0),
        fs,
        Droop::Rx {
            adc_hz: ADC_HZ_HF,
            r2,
        },
    );
    let mut ols = OverlapSave::new(&taps);
    let mut am = Am::new(fs);
    let mut buf = vec![Iq32::default(); 24_000 * n];
    r.read_rx(&mut buf).unwrap();
    let ch: Vec<C32> = (0..24_000).map(|f| rx_to_c32(buf[f * n])).collect();
    let mut y = Vec::new();
    ols.process(&ch, &mut y);
    let mut audio = Vec::new();
    am.process(&y, &mut audio);
    let tail = &audio[audio.len() - 8192..];
    let carrier = am.carrier() as f64;
    assert!(
        (db(carrier) + 20.0).abs() < 0.05,
        "carrier {:.3}",
        db(carrier)
    );
    let depth = real_amp(tail, 1000.0, fs) / carrier;
    assert!((depth - 0.5).abs() < 0.005, "depth {depth}");
}
