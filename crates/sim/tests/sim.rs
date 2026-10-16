// SPDX-License-Identifier: GPL-3.0-or-later
use num_complex::Complex64;
use std::time::Duration;
use web888_radio::*;
use web888_sim::*;

const F0: f64 = 7_074_000.0;

fn radio(scene: Scene, pacing: Pacing) -> SimRadio {
    let mut r = SimRadio::new(SimConfig::hf(scene, pacing));
    r.set_rx_rate(20).unwrap(); // 24 kS/s
    r
}

/// Read `frames` frames and return channel `ch` as complex samples.
fn grab(r: &mut SimRadio, ch: usize, frames: usize) -> Vec<Complex64> {
    let n = r.caps().rx_channels;
    let mut buf = vec![Iq32::default(); frames * n];
    let got = r.read_rx(&mut buf).unwrap();
    assert_eq!(got, frames);
    (0..frames)
        .map(|f| {
            let s = buf[f * n + ch];
            Complex64::new(s.i as f64, s.q as f64)
        })
        .collect()
}

fn mean_freq(z: &[Complex64], fs: f64) -> f64 {
    let mut acc = Complex64::new(0.0, 0.0);
    for w in z.windows(2) {
        acc += w[1] * w[0].conj();
    }
    acc.arg() * fs / (2.0 * std::f64::consts::PI)
}

fn mean_mag(z: &[Complex64]) -> f64 {
    z.iter().map(|c| c.norm()).sum::<f64>() / z.len() as f64
}

#[test]
fn tone_level_and_offset() {
    let mut r = radio(Scene::new().tone(F0 + 1000.0, -20.0), Pacing::Free);
    r.tune_rx(3, F0).unwrap();
    let z = grab(&mut r, 3, 4096);
    let fs_out = rx_rate(ADC_HZ_HF, 20);
    let f = mean_freq(&z, fs_out);
    assert!((f - 1000.0).abs() < 0.01, "offset {f}");
    let want = RX_SINE_FULL_SCALE * 0.1 * rx_chain_mag(1000.0, ADC_HZ_HF, 20);
    let got = mean_mag(&z);
    assert!((got / want - 1.0).abs() < 1e-6, "{got} vs {want}");
    // untuned channels see nothing
    let z0 = grab(&mut r, 0, 16);
    assert!(mean_mag(&z0) < 1.0);
}

#[test]
fn negative_offset_and_alias() {
    // a tone 0.7 * fs_out above the NCO aliases to -0.3 * fs_out, attenuated by the CIC
    let fs_out = rx_rate(ADC_HZ_HF, 20);
    let d = 0.7 * fs_out;
    let mut r = radio(Scene::new().tone(F0 + d, 0.0), Pacing::Free);
    r.tune_rx(0, F0).unwrap();
    let z = grab(&mut r, 0, 4096);
    let f = mean_freq(&z, fs_out);
    assert!((f + 0.3 * fs_out).abs() < 0.05, "{f}");
    let want = RX_SINE_FULL_SCALE * rx_chain_mag(d, ADC_HZ_HF, 20);
    assert!((mean_mag(&z) / want - 1.0).abs() < 1e-5);
}

#[test]
fn clock_error_and_correction() {
    let mut cfg = SimConfig::hf(Scene::new().tone(10e6, -10.0), Pacing::Free);
    cfg.adc_ppm = 10.0;
    let mut r = SimRadio::new(cfg);
    r.set_rx_rate(20).unwrap();
    let fs_true = ADC_HZ_HF * (1.0 + 10e-6);
    let fs_out = rx_rate(fs_true, 20);
    // NCO computed from the nominal rate runs 10 ppm fast: tone at -100 Hz
    r.tune_rx(0, 10e6).unwrap();
    let f = mean_freq(&grab(&mut r, 0, 4096), fs_out);
    assert!((f + 100.0).abs() < 0.01, "{f}");
    // after PPS correction the tone is back at 0 Hz
    r.set_adc_hz(fs_true);
    r.tune_rx(0, 10e6).unwrap();
    let f = mean_freq(&grab(&mut r, 0, 4096), fs_out);
    assert!(f.abs() < 0.01, "{f}");
}

#[test]
fn pps_free_running() {
    let mut cfg = SimConfig::hf(Scene::new(), Pacing::Free);
    cfg.adc_ppm = -2.5;
    let mut r = SimRadio::new(cfg);
    r.set_rx_rate(40).unwrap(); // 12 kS/s
    assert_eq!(r.pps().unwrap(), None);
    let n = r.caps().rx_channels;
    let mut buf = vec![Iq32::default(); 18_000 * n]; // 1.5 s
    r.read_rx(&mut buf).unwrap();
    let want = (ADC_HZ_HF * (1.0 - 2.5e-6)).round() as u32;
    assert_eq!(r.pps().unwrap(), Some(want));
    assert_eq!(r.pps().unwrap(), None);
}

#[test]
fn realtime_rate_and_overrun() {
    let mut r = radio(Scene::new(), Pacing::RealTime);
    let n = r.caps().rx_channels;
    let mut buf = vec![Iq32::default(); 20_000 * n];
    std::thread::sleep(Duration::from_millis(100));
    let got = r.read_rx(&mut buf).unwrap() as f64;
    // 24 kS/s * 0.1 s = 2400 frames (generous bounds for loaded CI hosts)
    assert!((2300.0..4000.0).contains(&got), "{got}");
    assert_eq!(r.stats().rx_overrun_frames, 0);
    // ring = 0x7FC00 / 96 = 5450 frames = 0.23 s; sleeping 0.5 s overruns
    std::thread::sleep(Duration::from_millis(500));
    let got = r.read_rx(&mut buf).unwrap();
    assert_eq!(got, 0x7FC00 / (8 * n));
    assert!(r.stats().rx_overrun_frames > 5000, "{:?}", r.stats());
}

#[test]
fn wf_one_shot_block() {
    let mut r = radio(Scene::new().tone(F0 + 250_000.0, -6.0), Pacing::Free);
    r.start_wf(1, F0, 16, WfMode::OneShot).unwrap();
    let mut buf = vec![Iq16::default(); 10_000];
    assert_eq!(r.read_wf(1, &mut buf).unwrap(), WF_BLOCK);
    assert_eq!(r.read_wf(1, &mut buf).unwrap(), 0);
    let z: Vec<Complex64> = buf[..WF_BLOCK]
        .iter()
        .map(|s| Complex64::new(s.i as f64, s.q as f64))
        .collect();
    let fs_out = ADC_HZ_HF / 16.0;
    assert!((mean_freq(&z, fs_out) - 250_000.0).abs() < 1.0);
    let want = WF_SINE_FULL_SCALE * 0.5012 * wf_chain_mag(250_000.0, ADC_HZ_HF, 16);
    assert!((mean_mag(&z) / want - 1.0).abs() < 0.01);
    assert!(matches!(
        r.start_wf(0, F0, 3, WfMode::OneShot),
        Err(Error::InvalidParam(_))
    ));
}

#[test]
fn noise_floor() {
    let mut r = radio(Scene::new().adc_noise(), Pacing::Free);
    let z = grab(&mut r, 0, 50_000);
    let p = z.iter().map(|c| c.norm_sqr()).sum::<f64>() / z.len() as f64;
    let fs_out = rx_rate(ADC_HZ_HF, 20);
    let want = 10f64.powf(scene_noise() / 10.0) * fs_out * RX_SINE_FULL_SCALE.powi(2);
    assert!((p / want - 1.0).abs() < 0.03, "{p} vs {want}");
}

fn scene_noise() -> f64 {
    Scene::new().adc_noise().noise_dbfs_hz.unwrap()
}

#[test]
fn attenuator_and_errors() {
    let mut r = radio(Scene::new().tone(F0, -10.0), Pacing::Free);
    r.tune_rx(0, F0 - 500.0).unwrap();
    let a = mean_mag(&grab(&mut r, 0, 256));
    r.front_end(FrontEnd {
        atten_db: 20.0,
        ..Default::default()
    })
    .unwrap();
    let b = mean_mag(&grab(&mut r, 0, 256));
    assert!((b / a - 0.1).abs() < 1e-6);
    let mut odd = vec![Iq32::default(); 13];
    assert!(matches!(r.read_rx(&mut odd), Err(Error::BadBuffer)));
    assert!(matches!(r.tune_rx(12, F0), Err(Error::InvalidChannel(12))));
    assert!(r.set_rx_rate(4).is_err());
    assert_eq!(signature_counts(r.identity().signature), (12, 2));
}
