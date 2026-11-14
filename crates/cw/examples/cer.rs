// SPDX-License-Identifier: GPL-3.0-or-later
//! Character error rates of the decoder on synthetic CW (the baseline a
//! neural decoder has to beat), and the skimmer's cost on audio.
//!
//!     cargo run --release -p web888-cw --example cer

use std::time::Instant;
use web888_cw::dsp::Rng;
use web888_cw::synth::{self, Channel, Keying};
use web888_cw::{AUDIO_RATE, DecodeOpts, ENV_FS, Skimmer, decode};

const MSGS: [&str; 5] = [
    "CQ CQ DE N0CALL N0CALL PSE K",
    "UR RST 579 579 NAME JOHN QTH OULU",
    "TNX FER QSO 73 ES GL DE N0CALL/P",
    "RIG IC7300 PWR 100W ANT DIPOLE",
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
    d[b.len()] as f64 / a.len().max(1) as f64
}

fn rate(k: &Keying, ch: &Channel, reps: u64) -> f64 {
    let mut s = 0.0;
    for rep in 0..reps {
        for (i, m) in MSGS.iter().enumerate() {
            let x = synth::complex(m, k, ch, ENV_FS, &mut Rng::new(rep * 100 + i as u64));
            s += cer(m, &decode(&x, &DecodeOpts::default()).text);
        }
    }
    s / (reps as f64 * MSGS.len() as f64)
}

fn main() {
    println!("SNR (2500 Hz)  clean 20 WPM  jitter 10 %  QSB 0.2 Hz");
    for snr in [6.0, 3.0, 0.0, -3.0, -6.0, -8.0, -10.0] {
        let ch = Channel {
            snr_db: Some(snr),
            ..Channel::new(20.0)
        };
        let a = rate(&Keying::new(20.0), &ch, 4);
        let b = rate(
            &Keying {
                jitter: 0.1,
                ..Keying::new(20.0)
            },
            &ch,
            4,
        );
        let c = rate(
            &Keying::new(20.0),
            &Channel {
                fade_hz: 0.2,
                ..ch.clone()
            },
            4,
        );
        println!(
            "{snr:>6.0} dB     {:>6.1} %      {:>6.1} %     {:>6.1} %",
            100.0 * a,
            100.0 * b,
            100.0 * c
        );
    }
    // skimmer: four signals, 60 s of audio
    let mut rng = Rng::new(1);
    let n = (60.0 * AUDIO_RATE) as usize;
    let mut audio = synth::audio_noise(n, AUDIO_RATE, 0.1, 6.0, &mut rng);
    for (j, hz) in [500.0, 800.0, 1200.0, 1900.0].iter().enumerate() {
        let s = synth::audio(
            MSGS[j],
            &Keying::new(16.0 + 6.0 * j as f64),
            &Channel::new(*hz),
            AUDIO_RATE,
            0.1,
            &mut rng,
        );
        for (i, a) in audio.iter_mut().enumerate() {
            *a += s[i % s.len()];
        }
    }
    let mut sk = Skimmer::default();
    let t = Instant::now();
    for c in audio.chunks(1024) {
        sk.push(c);
        for e in sk.poll() {
            if std::env::var("EV").is_ok() {
                println!(
                    "{:6.1} id {} {:6.1} Hz {:4.1} WPM {:+5.1} dB {:?}",
                    e.t, e.id, e.hz, e.wpm, e.snr, e.text
                );
            }
        }
    }
    let el = t.elapsed().as_secs_f64();
    println!(
        "skimmer: 60 s of audio, {} signals: {:.2} s ({:.1} % of real time)",
        sk.active().len(),
        el,
        el / 60.0 * 100.0
    );
}
