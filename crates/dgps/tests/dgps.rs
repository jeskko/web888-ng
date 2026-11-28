// SPDX-License-Identifier: GPL-3.0-or-later
use web888_cw::dsp::Rng;
use web888_dgps::rtcm::{Framer, check, encode, geodetic};
use web888_dgps::synth::{audio, bits, message, text_words};
use web888_dgps::{Body, Event, Skimmer};

#[test]
fn parity_round_trip() {
    let mut r = Rng::new(1);
    for _ in 0..2000 {
        let d = (r.next_u64() & 0xff_ffff) as u32;
        for (p29, p30) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
            let w = encode(d, p29, p30);
            assert_eq!(check(w, p29, p30), Some(d));
            // one bit flipped anywhere fails
            for b in 0..30 {
                assert_eq!(check(w ^ (1 << b), p29, p30), None);
            }
        }
    }
}

/// ECEF (m) of a position (WGS84 degrees, metres).
fn ecef(lat: f64, lon: f64, h: f64) -> (f64, f64, f64) {
    let a = 6_378_137.0;
    let f = 1.0 / 298.257_223_563;
    let e2 = f * (2.0 - f);
    let (la, lo) = (lat.to_radians(), lon.to_radians());
    let n = a / (1.0 - e2 * la.sin().powi(2)).sqrt();
    (
        (n + h) * la.cos() * lo.cos(),
        (n + h) * la.cos() * lo.sin(),
        (n * (1.0 - e2) + h) * la.sin(),
    )
}

/// Type 3's data words for a position (X, Y, Z: 32 bits each, cm).
fn pos_words(lat: f64, lon: f64, h: f64) -> Vec<u32> {
    let (x, y, z) = ecef(lat, lon, h);
    let v: Vec<u32> = [x, y, z]
        .iter()
        .map(|c| (c * 100.0).round() as i64 as u32)
        .collect();
    let bits: Vec<bool> = v
        .iter()
        .flat_map(|c| (0..32).rev().map(move |i| (c >> i) & 1 == 1))
        .collect();
    bits.chunks(24)
        .map(|c| c.iter().fold(0u32, |a, &b| (a << 1) | u32::from(b)))
        .collect()
}

fn stream() -> Vec<bool> {
    let mut w = Vec::new();
    for k in 0..4 {
        w.extend(message(16, 421, 100 + k, k, 0, &text_words("TEST N0CALL")));
        w.extend(message(
            3,
            421,
            101 + k,
            k,
            0,
            &pos_words(60.25, 24.5, 30.0),
        ));
        w.extend(message(6, 421, 102 + k, k, 0, &[]));
    }
    // some idle bits first, so the framer has to find the start
    let mut r = Rng::new(7);
    let mut b: Vec<bool> = (0..47).map(|_| r.next_u64() & 1 == 1).collect();
    b.extend(bits(&w));
    b
}

fn frame(b: &[bool]) -> Vec<web888_dgps::Msg> {
    let mut f = Framer::new(0.01);
    b.iter()
        .enumerate()
        .filter_map(|(i, &x)| f.push(x, i as f64 * 0.01))
        .collect()
}

#[test]
fn framer_decodes_text_position_and_either_polarity() {
    let b = stream();
    let inv: Vec<bool> = b.iter().map(|x| !x).collect();
    for s in [b, inv] {
        let m = frame(&s);
        assert_eq!(m.len(), 12, "{m:?}");
        assert!(m.iter().all(|x| x.complete() && x.station == 421));
        assert_eq!(m[0].body, Body::Text("TEST N0CALL".into()));
        assert!((m[0].z - 60.0).abs() < 1e-9);
        let Body::Pos { lat, lon, h } = m[1].body else {
            panic!("{:?}", m[1].body)
        };
        assert!((lat - 60.25).abs() < 1e-6 && (lon - 24.5).abs() < 1e-6 && (h - 30.0).abs() < 0.05);
        assert_eq!(m[2].kind, 6);
        assert!(m[2].words.is_empty());
    }
}

#[test]
fn geodetic_inverts_ecef() {
    for (lat, lon, h) in [(60.0, 25.0, 10.0), (-33.9, 151.2, 50.0), (64.9, -40.2, 0.0)] {
        let (x, y, z) = ecef(lat, lon, h);
        let (a, b, c) = geodetic(x, y, z);
        assert!((a - lat).abs() < 1e-8 && (b - lon).abs() < 1e-8 && (c - h).abs() < 1e-3);
    }
}

/// Two beacons 500 Hz apart (100 and 200 Bd) in noise: both found, their
/// messages read.
#[test]
fn skimmer_finds_two_beacons_in_noise() {
    let fs = 12_000.0;
    let mut words = Vec::new();
    for k in 0..40 {
        words.extend(message(16, 100 + (k % 2), k, k, 0, &text_words("HELLO")));
    }
    let b = bits(&words);
    let a1 = audio(&b, 100.0, 1000.0, fs, 0.1, 0.3);
    let a2 = audio(&b, 200.0, 1500.0, fs, 0.1, 1.1);
    let mut r = Rng::new(3);
    // SNR per beacon in 2500 Hz: 0 dB (0.1²/2 signal, noise σ² 0.005)
    let x: Vec<f32> = (0..a1.len())
        .map(|i| a1[i] + a2.get(i).copied().unwrap_or(0.0) + 0.0707 * r.normal() as f32)
        .collect();
    let mut sk = Skimmer::new(300.0, 2800.0);
    let mut locks = Vec::new();
    let mut texts = [0, 0];
    for c in x.chunks(1200) {
        sk.push(c);
        for e in sk.poll() {
            match e {
                Event::Lock { hz, baud, .. } => locks.push((hz, baud)),
                Event::Msg { hz, msg, .. } => {
                    if msg.body == Body::Text("HELLO".into()) {
                        texts[usize::from(hz > 1250.0)] += 1;
                    }
                }
            }
        }
    }
    locks.sort_by(|a, b| a.0.total_cmp(&b.0));
    assert_eq!(locks.len(), 2, "{locks:?}");
    assert!(
        (locks[0].0 - 1000.0).abs() < 1.0 && locks[0].1 == 100.0,
        "{locks:?}"
    );
    assert!(
        (locks[1].0 - 1500.0).abs() < 1.0 && locks[1].1 == 200.0,
        "{locks:?}"
    );
    // 100 Bd: 40 messages in 12 s (the first 8 s primed); 200 Bd in 6 s
    assert!(texts[0] >= 30 && texts[1] >= 30, "{texts:?}");
}
