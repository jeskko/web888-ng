// SPDX-License-Identifier: GPL-3.0-or-later
//! The audio chain on simulated hardware channels at 48 kS/s.

use num_complex::Complex64;
use web888_dsp::audio::{AUDIO_RATE, AudioChain, AudioSpec, Mode};
use web888_dsp::chan::Droop;
use web888_radio::*;
use web888_sim::*;

const F0: f64 = 7_074_000.0;

/// Amplitude of the real tone at `f` (Blackman-Harris windowed DFT).
fn amp(x: &[f32], f: f64) -> f64 {
    let n = x.len();
    let (a0, a1, a2, a3) = (0.35875, 0.48829, 0.14128, 0.01168);
    let mut acc = Complex64::new(0.0, 0.0);
    let mut wsum = 0.0;
    for (k, &v) in x.iter().enumerate() {
        let t = 2.0 * std::f64::consts::PI * k as f64 / (n - 1) as f64;
        let w = a0 - a1 * t.cos() + a2 * (2.0 * t).cos() - a3 * (3.0 * t).cos();
        acc += Complex64::from_polar(
            v as f64 * w,
            -2.0 * std::f64::consts::PI * f * k as f64 / AUDIO_RATE,
        );
        wsum += w;
    }
    2.0 * acc.norm() / wsum
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-12).log10()
}

/// Run `spec` on channel 0 tuned to F0 for `secs`; returns the audio after
/// the first 0.5 s and the power reading.
fn run(scene: Scene, spec: AudioSpec, secs: f64) -> (Vec<f32>, f32) {
    let mut r = SimRadio::new(SimConfig::hf(scene, Pacing::Free));
    r.set_rx_rate(10).unwrap(); // 48 kS/s
    r.tune_rx(0, F0).unwrap();
    let n = r.caps().rx_channels;
    let fs = rx_rate(ADC_HZ_HF, 10);
    let mut chain = AudioChain::new(
        spec,
        fs,
        Droop::Rx {
            adc_hz: ADC_HZ_HF,
            r2: 10,
        },
    );
    let mut buf = vec![Iq32::default(); 480 * n];
    let mut out = Vec::new();
    let mut frames = 0;
    let mut p = None;
    while (frames as f64) < secs * fs {
        let got = r.read_rx(&mut buf).unwrap();
        let block = RxBlock {
            first_frame: 0,
            nch: n,
            iq: buf[..got * n].to_vec(),
            overload: false,
        };
        chain.process(block.channel(0), &mut out);
        frames += got;
        if (frames as f64) > 0.5 * fs {
            p = chain.take_power_db().or(p);
        }
    }
    (out[6000..].to_vec(), p.unwrap())
}

fn fixed(mode: Mode) -> AudioSpec {
    AudioSpec {
        agc: None,
        ..AudioSpec::new(mode)
    }
}

#[test]
fn usb_hears_upper_sideband_only() {
    let scene = Scene::new()
        .tone(F0 + 1500.0, -40.0)
        .tone(F0 - 1000.0, -40.0);
    let (a, p) = run(scene, fixed(Mode::Usb), 1.5);
    let want = db(amp(&a, 1500.0));
    // -40 dBFS sine: |z| 0.01 complex -> real part amplitude 0.01
    assert!((want + 40.0).abs() < 0.2, "{want:.2}");
    assert!(db(amp(&a, 1000.0)) < want - 70.0);
    assert!((p + 40.0).abs() < 0.3, "power {p}");
}

#[test]
fn lsb_mirrors() {
    let scene = Scene::new()
        .tone(F0 + 1500.0, -40.0)
        .tone(F0 - 1000.0, -40.0);
    let (a, _) = run(scene, fixed(Mode::Lsb), 1.5);
    let want = db(amp(&a, 1000.0));
    assert!((want + 40.0).abs() < 0.2, "{want:.2}");
    assert!(db(amp(&a, 1500.0)) < want - 70.0);
}

#[test]
fn cw_pitch_and_narrow_filter() {
    // 100 Hz above the dial -> pitch + 100; 600 Hz above is outside +-250
    let scene = Scene::new().tone(F0 + 100.0, -60.0).tone(F0 + 600.0, -40.0);
    let (a, _) = run(scene, fixed(Mode::Cw), 1.5);
    let want = db(amp(&a, 800.0));
    assert!((want + 60.0).abs() < 0.3, "{want:.2}");
    assert!(db(amp(&a, 1300.0)) < -110.0, "{:.1}", db(amp(&a, 1300.0)));
}

#[test]
fn am_demodulates_with_agc() {
    let scene = Scene::new().am(F0, -50.0, 1000.0, 0.5);
    let (a, p) = run(scene, AudioSpec::new(Mode::Am), 2.0);
    let tone = amp(&a[6000..], 1000.0);
    // the AGC follows the demodulated audio's peaks (not the carrier)
    let target = 0.3;
    assert!((tone / target - 1.0).abs() < 0.05, "tone {tone}");
    assert!(p < -45.0 && p > -52.0, "power {p}");
}

#[test]
fn spec_changes_take_effect() {
    let mut chain = AudioChain::new(fixed(Mode::Usb), 48_000.0, Droop::None);
    let mut s = AudioSpec::new(Mode::Am);
    s.lo_hz = -9000.0;
    s.hi_hz = 9000.0;
    chain.set_spec(s);
    assert_eq!((chain.spec().lo_hz, chain.spec().hi_hz), (-5000.0, 5000.0));
    assert!(chain.gain_db() > 90.0, "AGC starts at max gain");
}
