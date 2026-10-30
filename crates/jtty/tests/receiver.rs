// SPDX-License-Identifier: GPL-3.0-or-later
//! jtty-cuf's receiver tests (tests/jtty_tests.cpp), and the stream: text
//! to audio, noise or a second signal added, what comes back.

use web888_jtty::{
    Message, MessageProfile, Receiver, modulate_message, pack_literal_message, receive_messages,
};

/// Deterministic Gaussian noise: splitmix64, Box-Muller.
struct Noise(u64, Option<f64>);

impl Noise {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn next(&mut self) -> f64 {
        if let Some(s) = self.1.take() {
            return s;
        }
        let u1 = ((self.next_u64() >> 11) as f64 + 1.0) / 9007199254740993.0;
        let u2 = (self.next_u64() >> 11) as f64 / 9007199254740992.0;
        let r = (-2.0 * u1.ln()).sqrt();
        let a = 2.0 * std::f64::consts::PI * u2;
        self.1 = Some(r * a.sin());
        r * a.cos()
    }
}

fn render(text: &str, hz: f64, amplitude: f64) -> Vec<f32> {
    let m = pack_literal_message(text, MessageProfile::Unknown).unwrap();
    modulate_message(&m.frames, hz, amplitude).unwrap()
}

fn mix(recording: &mut [f32], signal: &[f32], start: i64, gain: f64) {
    for (i, &v) in signal.iter().enumerate() {
        let at = start + i as i64;
        if at >= 0 && (at as usize) < recording.len() {
            recording[at as usize] += (gain * v as f64) as f32;
        }
    }
}

/// The gain that puts `signal` at `snr_db` over the noise in 2500 Hz.
fn gain_for_snr(signal: &[f32], snr_db: f64, noise_rms: f64) -> f64 {
    let power = signal.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / signal.len() as f64;
    let in_band = noise_rms * noise_rms / 6000.0 * 2500.0;
    (10f64.powf(snr_db / 10.0) * in_band / power).sqrt()
}

fn add_noise(recording: &mut [f32], rms: f64, seed: u64) {
    let mut n = Noise(seed, None);
    for v in recording {
        *v += (rms * n.next()) as f32;
    }
}

fn decoded(results: &[Message], text: &str, hz: i32) -> bool {
    results
        .iter()
        .any(|m| m.text == text && (m.frequency_hz - hz).abs() <= 3)
}

#[test]
fn clean_round_trips() {
    for hz in [300.0, 1500.0, 2800.0] {
        for text in [
            "HELLO",
            "K1ABC HELLO",
            "CQ K1ABC FN42",
            "HELLO, WORLD! 73 DE K1ABC",
        ] {
            let signal = render(text, hz, 0.5);
            let mut recording = vec![0f32; signal.len() + 20_000];
            mix(&mut recording, &signal, 7000, 1.0);
            let r = receive_messages(&recording).unwrap();
            assert_eq!(r.len(), 1, "{text} at {hz} Hz: {r:?}");
            assert!(decoded(&r, text, hz as i32), "{text} at {hz} Hz: {r:?}");
            assert!((r[0].start_sample - 7000).abs() < 100, "{r:?}");
        }
    }
}

#[test]
fn longest_message() {
    let text = "THE QUICK BROWN FOX JUMPS OVER THE LAZY DOG 0123456789 THE QUICK BROWN FOX JUMPS";
    let frames = pack_literal_message(text, MessageProfile::Unknown)
        .unwrap()
        .frames
        .len();
    assert!((10..=16).contains(&frames));
    let mut recording = vec![0f32; 22_656 * frames + 30_000];
    mix(&mut recording, &render(text, 1200.0, 0.5), 5000, 1.0);
    let r = receive_messages(&recording).unwrap();
    assert!(decoded(&r, text, 1200), "{r:?}");
}

#[test]
fn noisy_recordings() {
    // about -10 dB: well above the limit (about -15 dB), every trial decodes
    let text = "K1ABC HELLO";
    let signal = render(text, 1500.0, 1.0);
    for seed in 1..=6u64 {
        let mut recording = vec![0f32; signal.len() + 60_000];
        mix(
            &mut recording,
            &signal,
            9000 + seed as i64 * 1337,
            gain_for_snr(&signal, -10.0, 0.1),
        );
        add_noise(&mut recording, 0.1, seed);
        let r = receive_messages(&recording).unwrap();
        assert!(decoded(&r, text, 1500), "seed {seed}: {r:?}");
    }
}

#[test]
fn two_signals() {
    let mut recording = vec![0f32; 150_000];
    mix(
        &mut recording,
        &render("K1ABC HELLO", 1000.0, 0.5),
        5000,
        1.0,
    );
    mix(
        &mut recording,
        &render("QRZ? DE N0CALL", 1900.0, 0.2),
        20_000,
        1.0,
    );
    add_noise(&mut recording, 0.02, 11);
    let r = receive_messages(&recording).unwrap();
    assert_eq!(r.len(), 2, "{r:?}");
    assert!(decoded(&r, "K1ABC HELLO", 1000));
    assert!(decoded(&r, "QRZ? DE N0CALL", 1900));
    assert!(r[0].start_sample <= r[1].start_sample);
}

#[test]
fn a_recording_that_starts_inside_a_message() {
    let signal = render("K1ABC HELLO", 1500.0, 0.5);
    let mut tail = signal[10_000..].to_vec();
    tail.resize(tail.len() + 12_000, 0.0);
    assert!(decoded(&receive_messages(&tail).unwrap(), "HELLO", 1500));
    // cut by less than about 0.25 s: still whole
    let mut slightly = signal[2000..].to_vec();
    slightly.resize(slightly.len() + 12_000, 0.0);
    assert!(decoded(
        &receive_messages(&slightly).unwrap(),
        "K1ABC HELLO",
        1500
    ));
}

#[test]
fn noise_alone_decodes_nothing() {
    for seed in 1..=10u64 {
        let mut recording = vec![0f32; 120_000];
        add_noise(&mut recording, 0.1, 100 + seed);
        assert_eq!(receive_messages(&recording).unwrap(), vec![], "seed {seed}");
    }
    assert_eq!(receive_messages(&vec![0f32; 50_000]).unwrap(), vec![]);
}

#[test]
fn errors_are_reported() {
    assert!(receive_messages(&[]).is_err());
    assert!(receive_messages(&vec![0f32; 1_440_001]).is_err());
    let mut bad = vec![0f32; 30_000];
    bad[1000] = f32::NAN;
    assert!(receive_messages(&bad).is_err());
    assert!(pack_literal_message("", MessageProfile::Unknown).is_err());
    let longer = pack_literal_message(&"a".repeat(200), MessageProfile::Unknown).unwrap();
    assert_eq!(longer.canonical_text, "A".repeat(80));
    assert_eq!(longer.frames.len(), 16);
    let hello = pack_literal_message("HELLO", MessageProfile::Unknown).unwrap();
    assert!(modulate_message(&hello.frames, 50.0, 0.5).is_err());
}

/// The stream, fed in small pieces, finds what one reception of the whole
/// recording finds; messages come out as they complete, not at the end.
#[test]
fn the_stream_in_pieces_matches_one_reception() {
    let mut recording = vec![0f32; 200_000];
    mix(
        &mut recording,
        &render("K1ABC HELLO", 1000.0, 0.5),
        5000,
        1.0,
    );
    mix(
        &mut recording,
        &render("QRZ? DE N0CALL", 1900.0, 0.2),
        60_000,
        1.0,
    );
    mix(
        &mut recording,
        &render("CQ CQ DE K1ABC", 700.0, 0.3),
        120_000,
        1.0,
    );
    add_noise(&mut recording, 0.02, 7);
    let whole = receive_messages(&recording).unwrap();
    assert_eq!(whole.len(), 3, "{whole:?}");
    let mut r = Receiver::new();
    let mut streamed = Vec::new();
    let mut first_seen_at = None;
    for (i, chunk) in recording.chunks(1000).enumerate() {
        let got = r.push(chunk);
        if !got.is_empty() && first_seen_at.is_none() {
            first_seen_at = Some((i + 1) * 1000);
        }
        streamed.extend(got);
    }
    streamed.extend(r.finish());
    streamed.sort_by(|a, b| {
        (a.start_sample, a.frequency_hz, &a.text).cmp(&(b.start_sample, b.frequency_hz, &b.text))
    });
    assert_eq!(streamed, whole);
    // the first message (2 frames from sample 5000) is out within about two
    // windows of its end
    assert!(
        first_seen_at.unwrap() < 5000 + 2 * 22_656 + 2 * 28_320,
        "{first_seen_at:?}"
    );
}
