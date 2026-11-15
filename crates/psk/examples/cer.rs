// SPDX-License-Identifier: GPL-3.0-or-later
//! PSK31/63/125 on synthetic signals: character errors by SNR (2500 Hz),
//! the skimmer finding the signal by itself, five runs each; and its cost.
//!
//!     cargo run --release -p web888-psk --example cer

use std::time::Instant;
use web888_cw::dsp::Rng;
use web888_psk::{Event, Skimmer, synth};

const TEXT: &str =
    "CQ CQ de N0CALL N0CALL pse k  the quick brown fox jumps over the lazy dog 0123456789 ";

/// Character errors of `h` against the best-matching stretch of `r`
/// (where the decoder started and stopped is not counted), per character
/// of `h`.
fn cer(r: &str, h: &str) -> f64 {
    let (a, b): (Vec<char>, Vec<char>) = (h.chars().collect(), r.chars().collect());
    // rows: h; columns: r, free to start and end anywhere in r
    let mut d = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        let prev = d.clone();
        d[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            d[j + 1] = (prev[j + 1] + 1)
                .min(d[j] + 1)
                .min(prev[j] + usize::from(ca != cb));
        }
    }
    *d.iter().min().unwrap_or(&0) as f64 / a.len().max(1) as f64
}

fn main() {
    let fs = 12_000.0;
    let sd = 0.01f64;
    let t = Instant::now();
    let mut secs = 0.0;
    for baud in [31.25, 62.5, 125.0] {
        let reps = (baud / 31.25) as usize * 2;
        let text = TEXT.repeat(reps);
        println!("{baud} Bd    SNR   found (of 5)   locked after   CER of the text read");
        for snr in [0.0, -5.0, -8.0, -10.0, -12.0, -14.0] {
            let pn = sd * sd * 2500.0 / 6000.0;
            let amp = (2.0 * pn * 10f64.powf(snr / 10.0)).sqrt() as f32;
            let (mut found, mut e, mut when) = (0, 0.0, 0.0);
            for seed in 0..5u64 {
                let mut rng = Rng::new(seed);
                let fc = 1000.0 + 37.0 * seed as f64;
                let s = synth::audio(&text, baud, fc, fs, amp, seed as f64);
                let x: Vec<f32> = (0..s.len() + 36_000)
                    .map(|i| s.get(i).copied().unwrap_or(0.0) + (sd * rng.normal()) as f32)
                    .collect();
                secs += x.len() as f64 / fs;
                let mut sk = Skimmer::new(200.0, 2800.0);
                let mut out = String::new();
                let mut ok = false;
                for c in x.chunks(1200) {
                    sk.push(c);
                    for ev in sk.poll() {
                        match ev {
                            Event::Lock { hz, baud: b, .. } => {
                                if !ok && (hz - fc).abs() < 1.0 && b == baud {
                                    ok = true;
                                    when += sk.now() - 1.0; // the signal starts 1 s in (the synth's lead)
                                }
                            }
                            Event::Text { hz, text, .. } if (hz - fc).abs() < 3.0 => {
                                out.push_str(&text)
                            }
                            _ => {}
                        }
                    }
                }
                found += usize::from(ok);
                e += if out.is_empty() {
                    1.0
                } else {
                    cer(&text, &out).min(1.0)
                };
            }
            println!(
                "        {snr:4.0} dB   {found}            {:5.1} s        {:5.1} %",
                when / found.max(1) as f64,
                100.0 * e / 5.0
            );
        }
    }
    println!(
        "skimmer: {secs:.0} s of audio in {:.2} s ({:.2} % of real time)",
        t.elapsed().as_secs_f64(),
        100.0 * t.elapsed().as_secs_f64() / secs
    );
}
