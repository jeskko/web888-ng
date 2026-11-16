// SPDX-License-Identifier: GPL-3.0-or-later
//! The receiver on a recording: complex IQ (cf32, or cs16 as the page's
//! Rec IQ saves), the station's carrier shifted to a 1 kHz tone in 12 kHz
//! audio as USB would give it. Prints each second's symbol (a line a
//! minute) and each decoded minute; with START (the UTC Unix time of the
//! first sample) also how far the minute's start is from that clock.
//!
//!     [START=unix_s] cargo run --release -p web888-timecode --example replay FILE RATE CENTRE_HZ STATION [CARRIER_HZ]
use num_complex::Complex32;
use web888_timecode::{Event, Receiver, Station};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 5 {
        eprintln!("usage: replay FILE RATE CENTRE_HZ STATION [CARRIER_HZ]");
        std::process::exit(2);
    }
    let rate: f64 = a[2].parse().expect("rate");
    let centre: f64 = a[3].parse().expect("centre");
    let st = Station::from_name(&a[4]).expect("station: DCF77, MSF, WWVB, JJY, WWV");
    let carrier: f64 = a
        .get(5)
        .map(|v| v.parse().expect("carrier"))
        .unwrap_or(st.carriers()[0]);
    let start: Option<f64> = std::env::var("START")
        .ok()
        .map(|v| v.parse().expect("START"));
    let raw = std::fs::read(&a[1]).expect("read");
    // .f32: the receiver's audio itself (12 kHz mono, the carrier at 1 kHz
    // or WWV in AM), as the page gets it
    if a[1].ends_with(".f32") {
        let audio: Vec<f32> = raw
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect();
        let tone = if st == Station::Wwv { 100.0 } else { 1000.0 };
        run(st, &audio, tone, None);
        return;
    }
    let x: Vec<Complex32> = if a[1].ends_with(".cs16") {
        raw.as_chunks::<4>()
            .0
            .iter()
            .map(|c| {
                Complex32::new(
                    i16::from_le_bytes([c[0], c[1]]) as f32,
                    i16::from_le_bytes([c[2], c[3]]) as f32,
                ) / 32768.0
            })
            .collect()
    } else {
        raw.as_chunks::<8>()
            .0
            .iter()
            .map(|c| {
                Complex32::new(
                    f32::from_le_bytes(c[..4].try_into().unwrap()),
                    f32::from_le_bytes(c[4..].try_into().unwrap()),
                )
            })
            .collect()
    };
    // the carrier to +1 kHz, a low-pass (windowed sinc, 5 kHz), to 12 kHz real
    let fs = 12_000.0;
    let dec = (rate / fs).round() as usize;
    // WWV/WWVH: AM (the carrier at 0, the audio its envelope); the others
    // USB with the carrier at 1 kHz
    let am = st == Station::Wwv;
    let shift = if am {
        -(carrier - centre)
    } else {
        1000.0 - (carrier - centre)
    };
    let taps = 8 * dec + 1;
    let h: Vec<f32> = (0..taps)
        .map(|i| {
            let k = i as f64 - (taps / 2) as f64;
            let c = 5000.0 / rate;
            let s = if k == 0.0 {
                2.0 * c
            } else {
                (2.0 * std::f64::consts::PI * c * k).sin() / (std::f64::consts::PI * k)
            };
            (s * (0.54 - 0.46 * (2.0 * std::f64::consts::PI * i as f64 / (taps - 1) as f64).cos()))
                as f32
        })
        .collect();
    let mut audio = Vec::with_capacity(x.len() / dec);
    let w = 2.0 * std::f64::consts::PI * shift / rate;
    let mixed: Vec<Complex32> = x
        .iter()
        .enumerate()
        .map(|(n, &v)| v * Complex32::new((w * n as f64).cos() as f32, (w * n as f64).sin() as f32))
        .collect();
    let mut n = 0;
    let mut dc = 0.0f32;
    while n + taps <= mixed.len() {
        let y: Complex32 = mixed[n..n + taps].iter().zip(&h).map(|(a, &b)| a * b).sum();
        if am {
            let e = y.norm();
            dc += (e - dc) * 0.001;
            audio.push(e - dc);
        } else {
            audio.push(y.re);
        }
        n += dec;
    }
    run(st, &audio, if am { 100.0 } else { 1000.0 }, start);
}

fn run(st: Station, audio: &[f32], tone: f64, start: Option<f64>) {
    let fs = 12_000.0;
    let mut rx = Receiver::new(st, fs, tone, 0.0);
    let mut line = String::new();
    let mut stats: Vec<(usize, char, char)> = Vec::new();
    let mut lvs: Vec<(char, Vec<f32>)> = Vec::new();
    for c in audio.chunks(1200) {
        rx.push(c);
        for e in rx.poll() {
            match e {
                Event::Second(s) => {
                    if let (Some(s0), Ok(_)) = (start, std::env::var("TRUTH")) {
                        // the symbol sent: by the clock (START + t)
                        let u = s0 + s.t + 0.5;
                        let m0 = (u / 60.0).floor() as i64 * 60;
                        let i = (u - m0 as f64) as usize;
                        let want = web888_timecode::encode(st, m0)[i.min(59)];
                        stats.push((i, want, s.sym));
                        lvs.push((want, s.levels.clone()));
                        if std::env::var("LEVELS").is_ok() && want != s.sym {
                            eprintln!(
                                "  {:7.2} s{i:02} sent {want} read {} q {:.2} levels {:?}",
                                s.t, s.sym, s.quality, s.levels
                            );
                        }
                    }
                    if s.index == Some(0) || line.is_empty() {
                        if !line.is_empty() {
                            println!("{line}");
                        }
                        line = format!("{:7.2} ", s.t);
                    }
                    line.push(s.sym);
                }
                Event::Minute(m) => {
                    println!("{line}");
                    line.clear();
                    let off = start
                        .map(|s0| {
                            format!(
                                " (heard {:+.3} s from START's clock)",
                                s0 + m.t - m.utc as f64
                            )
                        })
                        .unwrap_or_default();
                    println!(
                        "MINUTE {} at {:.2} s: {} {} {}{off}",
                        st.name(),
                        m.t,
                        m.local,
                        if m.confirmed {
                            "confirmed"
                        } else if m.ok {
                            "ok"
                        } else {
                            "CHECKS FAILED"
                        },
                        m.notes.join(", ")
                    );
                }
            }
        }
    }
    println!("{line}");
    eprintln!("level {:.1} dB", rx.level);
    if !stats.is_empty() {
        // DCF77's bits 1-14 (weather) and MSF's DUT1 aren't known here
        let skip = |i: usize| match st {
            Station::Dcf77 => (1..=15).contains(&i),
            Station::Msf => (1..=16).contains(&i),
            _ => false,
        };
        let s: Vec<_> = stats.iter().filter(|x| !skip(x.0)).collect();
        let bad = s.iter().filter(|x| x.1 != x.2).count();
        eprintln!(
            "symbols: {} of {} wrong ({:.1} %)",
            bad,
            s.len(),
            100.0 * bad as f64 / s.len() as f64
        );
        let mut conf = std::collections::BTreeMap::new();
        for x in &s {
            if x.1 != x.2 {
                *conf.entry(format!("{}->{}", x.1, x.2)).or_insert(0) += 1;
            }
        }
        eprintln!("  sent->read: {conf:?}");
        for c in ['0', '1', 'M'] {
            let v: Vec<&Vec<f32>> = lvs.iter().filter(|x| x.0 == c).map(|x| &x.1).collect();
            if v.is_empty() {
                continue;
            }
            let k = v[0].len();
            let mut med = Vec::new();
            for j in 0..k {
                let mut col: Vec<f32> = v.iter().map(|x| x[j]).collect();
                col.sort_by(f32::total_cmp);
                med.push((col[col.len() / 2] * 100.0).round() / 100.0);
            }
            eprintln!("  sent {c}: median levels {med:?} ({} s)", v.len());
        }
    }
}
