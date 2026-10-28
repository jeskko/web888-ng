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
    // carrier-referenced: 100 % modulation peaks at the target, so 50 %
    // gives half of it
    let target = 0.3;
    assert!((tone / (0.5 * target) - 1.0).abs() < 0.05, "tone {tone}");
    assert!(p < -45.0 && p > -52.0, "power {p}");
}

#[test]
fn am_agc_keeps_the_modulation_depth_at_any_level() {
    // 50 % at -70 dBFS and 25 % at -20 dBFS: the same loudness for the
    // same depth, and the shallower one 6 dB quieter
    let (weak, _) = run(
        Scene::new().am(F0, -70.0, 1000.0, 0.5),
        AudioSpec::new(Mode::Am),
        2.0,
    );
    let (strong, _) = run(
        Scene::new().am(F0, -20.0, 1000.0, 0.25),
        AudioSpec::new(Mode::Am),
        2.0,
    );
    let w = db(amp(&weak[6000..], 1000.0));
    let s = db(amp(&strong[6000..], 1000.0));
    assert!((w - db(0.15)).abs() < 0.5, "weak {w:.2} dB");
    assert!(
        (w - s - 6.02).abs() < 0.5,
        "depth difference {:.2} dB",
        w - s
    );
}

#[test]
fn noise_blanker_restores_a_tone_under_impulses() {
    // USB: a -100 dBFS tone at +1 kHz in noise (about 15 dB SNR in the
    // passband), plus 100 Hz impulses 40 dB over the tone, a few samples
    // long as they leave the hardware CIC
    let fs: f64 = 48_000.0;
    let secs = 3.0;
    let n = (fs * secs) as usize;
    let mut rng = 0x1234_5678u64;
    let mut gauss = || {
        let mut u = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            ((rng >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        };
        let (a, b) = (u(), u());
        let r = (-2.0 * a.ln()).sqrt();
        (
            r * (2.0 * std::f64::consts::PI * b).cos(),
            r * (2.0 * std::f64::consts::PI * b).sin(),
        )
    };
    let tone_a = 10f64.powf(-100.0 / 20.0);
    let noise_rms = tone_a * 10f64.powf(-15.0 / 20.0) * (fs / 2400.0).sqrt() / 2f64.sqrt();
    let full = RX_SINE_FULL_SCALE;
    let clean: Vec<Iq32> = (0..n)
        .map(|k| {
            let ph = 2.0 * std::f64::consts::PI * 1000.0 * k as f64 / fs;
            let (gi, gq) = gauss();
            Iq32 {
                i: ((tone_a * ph.cos() + noise_rms * gi) * full) as i32,
                q: ((tone_a * ph.sin() + noise_rms * gq) * full) as i32,
            }
        })
        .collect();
    let mut noisy = clean.clone();
    for k in (0..n).step_by(480) {
        for (j, a) in [0.5, 1.0, 0.7, 0.3].iter().enumerate() {
            if k + j < n {
                noisy[k + j].i += (a * 100.0 * tone_a * full) as i32;
                noisy[k + j].q -= (a * 60.0 * tone_a * full) as i32;
            }
        }
    }
    let snr = |iq: &[Iq32], nb: Option<f32>| {
        let mut spec = fixed(Mode::Usb);
        spec.gain_db = 60.0;
        spec.nb = nb;
        let mut chain = AudioChain::new(spec, fs, Droop::None);
        let mut out = Vec::new();
        for c in iq.chunks(240) {
            chain.process(c.iter().copied(), &mut out);
        }
        let a = &out[6000..];
        let t = amp(a, 1000.0);
        let total = a.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / a.len() as f64;
        db(t / 2f64.sqrt()) - 0.5 * db(total - t * t / 2.0)
    };
    let clean_snr = snr(&clean, None);
    let hit = snr(&noisy, None);
    let blanked = snr(&noisy, Some(8.0));
    eprintln!("SNR: clean {clean_snr:.1} dB, impulses {hit:.1} dB, blanked {blanked:.1} dB");
    assert!(
        hit < clean_snr - 10.0,
        "impulses: {hit:.1} dB (clean {clean_snr:.1})"
    );
    assert!(
        blanked > clean_snr - 3.0,
        "blanked: {blanked:.1} dB (clean {clean_snr:.1}, hit {hit:.1})"
    );
}

#[test]
fn squelch_mutes_a_quiet_channel() {
    let mut spec = AudioSpec::new(Mode::Am);
    spec.squelch_db = Some(-60.0);
    let (a, _) = run(Scene::new().adc_noise(), spec, 1.0);
    assert!(a.iter().all(|&x| x == 0.0), "closed on noise");
    let (a, _) = run(Scene::new().am(F0, -40.0, 1000.0, 0.5), spec, 1.0);
    assert!(amp(&a, 1000.0) > 0.1, "open on a signal");
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

/// A carrier at F0 with a tone 1500 Hz above and one 1000 Hz below.
fn two_sidebands() -> Scene {
    Scene::new()
        .tone(F0, -40.0)
        .tone(F0 + 1500.0, -50.0)
        .tone(F0 - 1000.0, -50.0)
}

#[test]
fn sar_and_sal_keep_one_sideband() {
    let (u, _) = run(two_sidebands(), AudioSpec::new(Mode::Sar), 2.0);
    let (l, _) = run(two_sidebands(), AudioSpec::new(Mode::Sal), 2.0);
    let (u, l) = (&u[6000..], &l[6000..]);
    // the tones are 10 dB under the carrier: 0.316 of it, doubled, x 0.3
    assert!(
        (amp(u, 1500.0) / 0.19 - 1.0).abs() < 0.05,
        "{}",
        amp(u, 1500.0)
    );
    assert!(
        (amp(l, 1000.0) / 0.19 - 1.0).abs() < 0.05,
        "{}",
        amp(l, 1000.0)
    );
    assert!(db(amp(u, 1000.0)) < -70.0, "{:.1}", db(amp(u, 1000.0)));
    assert!(db(amp(l, 1500.0)) < -70.0, "{:.1}", db(amp(l, 1500.0)));
}

#[test]
fn sas_is_stereo_lower_left() {
    let mut chain = AudioChain::new(AudioSpec::new(Mode::Sas), 48_000.0, Droop::None);
    assert_eq!(chain.channels(), 2);
    let (a, _) = run(two_sidebands(), AudioSpec::new(Mode::Sas), 2.0);
    let (l, r): (Vec<f32>, Vec<f32>) = a[12_000..].chunks(2).map(|f| (f[0], f[1])).unzip();
    assert!(db(amp(&l, 1000.0)) - db(amp(&l, 1500.0)) > 40.0);
    assert!(db(amp(&r, 1500.0)) - db(amp(&r, 1000.0)) > 40.0);
    // a mono mode again: one channel
    chain.set_spec(AudioSpec::new(Mode::Sam));
    assert_eq!(chain.channels(), 1);
}
