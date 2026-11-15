// SPDX-License-Identifier: GPL-3.0-or-later
//! Fading and auroral flutter: character errors of the classical and the
//! neural decoder on one signal (20 and 30 WPM, 5 % jitter) under Rayleigh
//! fading at Doppler spreads from slow QSB (0.2 Hz) to auroral flutter
//! (2-20 Hz: the carrier spread over tens of Hz, the level changing within
//! a dot). The network was trained with 0.03-0.6 Hz only; this says
//! whether flutter is worth training for.
//!
//!     cargo run --release -p web888-cw --example flutter [reps]

use web888_cw::dsp::Rng;
use web888_cw::synth::{self, Channel, Keying};
use web888_cw::{DecodeOpts, ENV_FS, decode, net};

const MSGS: [&str; 5] = [
    "CQ CQ DE N0CALL N0CALL PSE K",
    "UR RST 579 579 NAME JOHN QTH OULU",
    "TNX FER QSO 73 ES GL DE N0CALL/P",
    "TEST N0ABC N0ABC 5NN 14 TU",
    "WX CLOUDY TEMP 5C = HW CPY?",
];

fn cer(r: &str, h: &str) -> f64 {
    let a: Vec<char> = r.chars().filter(|&c| c != ' ').collect();
    let b: Vec<char> = h.chars().filter(|&c| c != ' ').collect();
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
    (d[b.len()] as f64 / a.len().max(1) as f64).min(1.0)
}

fn main() {
    let reps: u64 = std::env::args()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    let dopplers = [0.0, 0.2, 2.0, 5.0, 10.0, 20.0];
    for wpm in [20.0, 30.0] {
        println!(
            "character errors, classical / neural ({wpm:.0} WPM; SNR in 2500 Hz; Doppler spread)"
        );
        print!("SNR  ");
        for d in dopplers {
            print!(
                "  {:>12}",
                if d == 0.0 {
                    "none".to_string()
                } else {
                    format!("{d} Hz")
                }
            );
        }
        println!();
        for snr in [20.0, 10.0, 5.0, 0.0, -5.0] {
            print!("{snr:>3.0} dB");
            for fade_hz in dopplers {
                let (mut a, mut b) = (0.0, 0.0);
                for rep in 0..reps {
                    for (i, m) in MSGS.iter().enumerate() {
                        let k = Keying {
                            jitter: 0.05,
                            ..Keying::new(wpm)
                        };
                        let ch = Channel {
                            snr_db: Some(snr),
                            fade_hz,
                            ..Channel::new(3.0)
                        };
                        let x =
                            synth::complex(m, &k, &ch, ENV_FS, &mut Rng::new(rep * 100 + i as u64));
                        a += cer(m, &decode(&x, &DecodeOpts::default()).text);
                        let t: String = net::decode(&x, 0.0).into_iter().map(|c| c.1).collect();
                        b += cer(m, &t);
                    }
                }
                let n = (reps as usize * MSGS.len()) as f64;
                print!("  {:>5.1} /{:>5.1}", 100.0 * a / n, 100.0 * b / n);
            }
            println!();
        }
    }
}
