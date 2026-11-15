// SPDX-License-Identifier: GPL-3.0-or-later
use num_complex::Complex32;
use std::collections::HashMap;
use web888_cw::dsp::Rng;
use web888_psk::demod::{Demod, SPS};
use web888_psk::synth;
use web888_psk::{Event, Skimmer};

const TEXT: &str = "CQ CQ de N0CALL N0CALL pse k ";

fn noise(n: usize, sd: f64, rng: &mut Rng) -> Vec<f32> {
    (0..n).map(|_| (sd * rng.normal()) as f32).collect()
}

/// White noise of `sd` per sample at 12 kS/s for an SNR in 2500 Hz.
fn amp_for(snr_db: f64, sd: f64) -> f32 {
    // a carrier of amplitude A has power A²/2 (the PSK envelope's mean
    // square is ~0.75 of it during text); noise in 2500 Hz: sd² 2500/6000
    let pn = sd * sd * 2500.0 / 6000.0;
    (2.0 * pn * 10f64.powf(snr_db / 10.0)).sqrt() as f32
}

#[test]
fn demod_reads_with_an_offset() {
    let baud = 31.25;
    let fs = baud * SPS as f64;
    let text = TEXT.repeat(2);
    let b = synth::baseband(&synth::symbols(&text), baud, fs);
    let off = 2.0;
    let x: Vec<Complex32> = b
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            Complex32::from_polar(
                v,
                (2.0 * std::f64::consts::PI * off * i as f64 / fs + 0.7) as f32,
            )
        })
        .collect();
    let mut d = Demod::new(baud, 0.0);
    let s: String = d.feed(&x).iter().map(|i| i.1).collect();
    assert!(s.contains(TEXT.trim()), "{s:?}");
    assert!((d.freq - off).abs() < 0.3, "{}", d.freq);
}

fn skim(x: &[f32]) -> (HashMap<u32, String>, HashMap<u32, (f64, f64)>) {
    let mut sk = Skimmer::new(200.0, 2800.0);
    let (mut text, mut lock) = (HashMap::new(), HashMap::new());
    for c in x.chunks(1200) {
        sk.push(c);
        for e in sk.poll() {
            match e {
                Event::Lock { id, hz, baud } => {
                    lock.insert(id, (hz, baud));
                }
                Event::Text { id, text: t, .. } => {
                    text.entry(id).or_insert_with(String::new).push_str(&t)
                }
            }
        }
    }
    (text, lock)
}

#[test]
fn three_rates_in_the_passband() {
    let fs = 12_000.0;
    let mut rng = Rng::new(3);
    let sd = 0.01;
    let sigs = [
        (31.25, 900.0, 3.0),
        (62.5, 1500.37, 6.0),
        (125.0, 2200.0, 9.0),
    ];
    let parts: Vec<Vec<f32>> = sigs
        .iter()
        .enumerate()
        .map(|(k, &(baud, fc, snr))| {
            synth::audio(&TEXT.repeat(4), baud, fc, fs, amp_for(snr, sd), k as f64)
        })
        .collect();
    let n = parts.iter().map(Vec::len).max().unwrap() + 24_000;
    let mut x = noise(n, sd, &mut rng);
    for p in &parts {
        for (a, b) in x.iter_mut().zip(p) {
            *a += b;
        }
    }
    let (text, lock) = skim(&x);
    for &(baud, fc, _) in &sigs {
        let (id, &(hz, _)) = lock
            .iter()
            .find(|(_, v)| (v.0 - fc).abs() < 1.0 && v.1 == baud)
            .unwrap_or_else(|| panic!("{baud} Bd at {fc} not locked: {lock:?}"));
        let t = text.get(id).cloned().unwrap_or_default();
        assert!(
            t.contains("de N0CALL N0CALL"),
            "{baud} Bd at {hz:.2}: {t:?}"
        );
    }
}

#[test]
fn noise_alone_gives_no_text() {
    let mut rng = Rng::new(9);
    let x = noise(12_000 * 30, 0.01, &mut rng);
    let (text, _) = skim(&x);
    let n: usize = text.values().map(|t| t.len()).sum();
    assert!(n < 5, "{text:?}");
}
