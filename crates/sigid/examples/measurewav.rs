// SPDX-License-Identifier: GPL-3.0-or-later
//! "What is this?" on 12 kHz audio (16-bit PCM WAV, mono, USB): the
//! measurements of the signal in an audio range and the identifier's
//! classes, from 4.096 s starting at T0.
//!
//!     [LSB=1] cargo run --release -p web888-sigid --example measurewav FILE.wav LO HI [T0 [BAND_LO BAND_HI]]

use web888_sigid::measure::NA;
use web888_sigid::scan::{AUDIO_RATE, analyse};

fn read_wav(path: &str) -> Vec<f32> {
    let b = std::fs::read(path).expect("read");
    let mut p = 12;
    let mut rate = 0u32;
    while p + 8 <= b.len() {
        let id = &b[p..p + 4];
        let len = u32::from_le_bytes(b[p + 4..p + 8].try_into().unwrap()) as usize;
        if id == b"fmt " {
            rate = u32::from_le_bytes(b[p + 12..p + 16].try_into().unwrap());
        }
        if id == b"data" {
            assert_eq!(rate as f64, AUDIO_RATE, "12 kHz audio only");
            return b[p + 8..(p + 8 + len).min(b.len())]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| i16::from_le_bytes(*c) as f32 / 32768.0)
                .collect();
        }
        p += 8 + len + (len & 1);
    }
    panic!("no data chunk");
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let f = |i: usize, d: f64| a.get(i).map_or(d, |v| v.parse().expect("number"));
    let x = read_wav(&a[1]);
    let (lo, hi, t0) = (f(2, 200.0), f(3, 2800.0), f(4, 0.0));
    let band = (f(5, 200.0), f(6, 2800.0));
    let s = ((t0 * AUDIO_RATE) as usize).min(x.len().saturating_sub(NA));
    let lsb = std::env::var("LSB").is_ok_and(|v| v == "1");
    match analyse(&x[s..(s + NA).min(x.len())], (lo, hi), band, lsb) {
        None => println!("{}: nothing over the floor", a[1]),
        Some(r) => {
            let top: Vec<String> = r
                .top
                .iter()
                .take(3)
                .map(|(c, p)| format!("{c} {:.0}%", 100.0 * p))
                .collect();
            if std::env::var("LONG").is_ok() {
                println!("{}: {}\n{:#?}", a[1], top.join(", "), r.m);
                return;
            }
            let m = &r.m;
            let o = |l: Option<web888_sigid::measure::Line>, u: &str| {
                l.map_or("-".into(), |l| format!("{:.2}{u}/{:.2}", l.at, l.db))
            };
            println!(
                "{}: {} | grid {} | {:.0}-{:.0} c{:.0} bw90 {:.0} snr {:.0}{} | duty {:.2} n{} short {:.3} cv {:.2} syl {:.2} papr {:.1} | tones {} sh {} sp {} line {:.2} sb {:.3}/{:.3} | bEnv {} bFrq {} psk {} | per {} envPer {} | drift {}",
                a[1].rsplit('/').next().unwrap(),
                top.join(", "),
                m.grid.map_or("-".into(), |g| format!(
                    "{:.2}Hz x{} r{:.2} T{:.0}ms ch{:.0}/s",
                    g.spacing,
                    g.n,
                    g.strength,
                    1000.0 * g.frame,
                    g.changes
                )),
                m.lo,
                m.hi,
                m.centre,
                m.bw90,
                m.snr,
                if m.fills { " FILLS" } else { "" },
                m.duty,
                m.bursts,
                m.on_short,
                m.env_cv,
                m.syllabic,
                m.papr,
                m.tones
                    .iter()
                    .take(6)
                    .map(|t| format!("{:.0}", t.at))
                    .collect::<Vec<_>>()
                    .join(","),
                m.shift.map_or("-".into(), |v| format!("{v:.0}")),
                m.spacing.map_or("-".into(), |v| format!("{v:.1}")),
                m.line,
                m.side_lo,
                m.side_hi,
                o(m.baud_env, ""),
                o(m.baud_freq, ""),
                m.psk
                    .map_or("-".into(), |(k, l)| format!("{k}/{:.2}", l.db)),
                o(m.period, "s"),
                o(m.env_period, "s"),
                m.drift.map_or("-".into(), |v| format!("{v:.2}")),
            );
        }
    }
}
