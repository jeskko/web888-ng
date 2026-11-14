// SPDX-License-Identifier: GPL-3.0-or-later
//! Ham RTTY (45.45 Bd, 170 Hz, 5N1.5) on synthetic signals: whether every
//! parameter is detected, and the character error rate with known
//! parameters, by SNR (2500 Hz); and the skimmer's cost on audio.
//!
//!     cargo run --release -p web888-fsk --example cer

use std::time::Instant;
use web888_cw::dsp::Rng;
use web888_fsk::analyze::{FskDecoder, FskParams, Parity, analyze, is_valid};
use web888_fsk::charset::ita2_encode;
use web888_fsk::skim::{AUDIO_RATE, Skimmer};
use web888_fsk::synth::{self, Cond};

const TEXT: &str = "CQ CQ TEST DE N0CALL N0CALL 599 001 KN THE QUICK BROWN FOX JUMPS OVER THE LAZY DOG 0123456789 ";

fn cer(r: &str, h: &str) -> f64 {
    let (a, b): (Vec<char>, Vec<char>) = (r.chars().collect(), h.chars().collect());
    let mut d: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let prev = d.clone();
        d[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            d[j + 1] = (prev[j + 1] + 1)
                .min(d[j] + 1)
                .min(prev[j] + usize::from(ca != cb));
        }
    }
    d[b.len()] as f64 / a.len().max(1) as f64
}

fn main() {
    let fs = 4000.0;
    let t = TEXT.repeat(3);
    let seg = synth::async_simple(&ita2_encode(&t, true), 5, Parity::None, 1.5);
    println!("SNR     detected (of 5)   CER, known parameters");
    for snr in [0.0, -3.0, -5.0, -7.0, -9.0] {
        let (mut ok, mut e) = (0, 0.0);
        for seed in 0..5 {
            let x = synth::complex(
                &seg,
                45.45,
                170.0,
                fs,
                &Cond {
                    snr_db: Some(snr),
                    ..Cond::new(0.0)
                },
                &mut Rng::new(seed),
            );
            let r = analyze(&x, fs, None, 0.0);
            if is_valid(&r, 12)
                && r.params.as_ref().is_some_and(|p| {
                    p.baud == 45.45 && p.framing() == "5N1.5" && p.charset == "ITA2"
                })
            {
                ok += 1;
            }
            let mut p = FskParams::new(-85.0, 85.0, 45.45);
            p.stop = 1.5;
            let txt: String = FskDecoder::new(fs, p, 0.0)
                .feed(&x)
                .iter()
                .map(|i| i.1.as_str())
                .collect();
            e += cer(&t, &txt);
        }
        println!(
            "{snr:>4.0} dB  {ok:>8}             {:>5.1} %",
            100.0 * e / 5.0
        );
    }
    let mut rng = Rng::new(1);
    let a = synth::audio(
        &seg,
        45.45,
        170.0,
        AUDIO_RATE,
        &Cond::new(900.0),
        0.1,
        &mut rng,
    );
    let x: Vec<f32> = a.iter().map(|v| v + (0.02 * rng.normal()) as f32).collect();
    let mut sk = Skimmer::default();
    let t0 = Instant::now();
    for c in x.chunks(1024) {
        sk.push(c);
        sk.poll();
    }
    let el = t0.elapsed().as_secs_f64();
    let dur = x.len() as f64 / AUDIO_RATE;
    println!(
        "skimmer: {dur:.0} s of audio, one signal: {el:.2} s ({:.1} % of real time)",
        100.0 * el / dur
    );
}
