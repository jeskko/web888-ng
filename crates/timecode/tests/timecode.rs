// SPDX-License-Identifier: GPL-3.0-or-later
//! Each station's keying on a 1 kHz tone with noise, through the receiver.
use web888_timecode::{Event, Receiver, Station, days_from_civil, keying};

const FS: f64 = 12_000.0;

struct Rng(u64);
impl Rng {
    fn uniform(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    fn normal(&mut self) -> f64 {
        (-2.0 * self.uniform().ln()).sqrt() * (2.0 * std::f64::consts::PI * self.uniform()).cos()
    }
}

/// Audio: the station's keying on a tone at `tone` Hz, starting `lead`
/// seconds into a minute, noise `sigma` (the tone's amplitude is 1).
fn audio(
    station: Station,
    start: i64,
    minutes: usize,
    tone: f64,
    lead: f64,
    sigma: f64,
    seed: u64,
) -> Vec<f32> {
    let k = keying(station, start, minutes);
    let skip = (lead * 1000.0) as usize;
    let mut rng = Rng(seed);
    let n = ((k.len() - skip) as f64 / 1000.0 * FS) as usize;
    (0..n)
        .map(|i| {
            let t = i as f64 / FS;
            let lv = k[skip + (t * 1000.0) as usize] as f64;
            (lv * (2.0 * std::f64::consts::PI * tone * t).sin() + sigma * rng.normal()) as f32
        })
        .collect()
}

fn run(station: Station, sigma: f64) -> Vec<(f64, i64, bool, String)> {
    let start = days_from_civil(2026, 11, 16) * 86_400 + 10 * 3600 + 20 * 60;
    let lead = 23.4; // start mid-minute, at an odd fraction of a second
    let x = audio(station, start, 5, 1000.0, lead, sigma, 7);
    let mut rx = Receiver::new(station, FS, 1000.0, 0.0);
    let mut out = Vec::new();
    for c in x.chunks(1200) {
        rx.push(c);
        for e in rx.poll() {
            if let Event::Minute(m) = e {
                // the stream time t of the minute's start is UTC - (start + lead)
                out.push((m.t, m.utc, m.ok, m.local.clone()));
                let want = (m.utc - start) as f64 - lead;
                assert!(
                    (m.t - want).abs() < 0.03,
                    "{station:?}: minute at {} s, wanted {want}",
                    m.t
                );
            }
        }
    }
    out
}

#[test]
fn every_station_decodes() {
    for st in Station::KEYED {
        let mins = run(st, 0.3);
        let good = mins.iter().filter(|m| m.2).count();
        assert!(good >= 3, "{st:?}: {mins:?}");
    }
}

#[test]
fn dcf77_in_noise() {
    // ~15 dB in 25 Hz: two of the three whole minutes pass their checks
    let mins = run(Station::Dcf77, 2.0);
    assert!(mins.iter().filter(|m| m.2).count() >= 2, "{mins:?}");
    assert!(
        mins.iter().any(|m| m.3 == "2026-11-16 11:23 CET"),
        "{mins:?}"
    );
}

#[test]
#[ignore]
fn debug_symbols() {
    let st = Station::from_name(&std::env::var("ST").unwrap_or("DCF77".into())).unwrap();
    let start = days_from_civil(2026, 11, 16) * 86_400 + 10 * 3600 + 20 * 60;
    let sigma: f64 = std::env::var("SIGMA")
        .map(|v| v.parse().unwrap())
        .unwrap_or(0.3);
    let x = audio(st, start, 5, 1000.0, 23.4, sigma, 7);
    let mut rx = Receiver::new(st, FS, 1000.0, 0.0);
    let mut line = String::new();
    for c in x.chunks(1200) {
        rx.push(c);
        for e in rx.poll() {
            match e {
                Event::Second(s) => {
                    if s.index == Some(0) || line.is_empty() {
                        eprintln!("{line}");
                        line = format!("{:7.2} ", s.t);
                    }
                    line.push(s.sym);
                }
                Event::Minute(m) => eprintln!("\nMIN {m:?}"),
            }
        }
    }
    eprintln!("{line}");
}

/// TDF's phase in the second at `t` (s into it) with its symbol: a cycle
/// of +-0.9 rad centred on 0 ms (+ before, - after: its first half is the
/// end of the second before), a 1's centred on 100 ms, none in second 59;
/// over 150-800 ms other modulation (here: a cycle every 50 ms, signs from
/// `busy`).
fn tdf_phase(sym: char, next: char, t: f64, busy: u64) -> f64 {
    let cycle = |t: f64| -> f64 {
        // -50..0 ms: +, 0..50 ms: -, triangles of 0.9 rad
        if (-0.05..0.0).contains(&t) {
            0.9 * (1.0 - (t + 0.025).abs() / 0.025)
        } else if (0.0..0.05).contains(&t) {
            -0.9 * (1.0 - (t - 0.025).abs() / 0.025)
        } else {
            0.0
        }
    };
    let mut p = 0.0;
    if sym != 'M' {
        p += cycle(t);
    }
    if sym == '1' {
        p += cycle(t - 0.1);
    }
    // the next second's marker begins in this one
    if next != 'M' {
        p += cycle(t - 1.0);
    }
    if (0.2..0.75).contains(&t) {
        let k = ((t - 0.2) / 0.05) as u64;
        if (busy >> (k % 64)) & 1 == 1 {
            p += 0.5 * cycle(t - 0.2 - k as f64 * 0.05 - 0.05);
        }
    }
    p
}

/// TDF phase-modulated on a 1 kHz tone, with noise and the other
/// modulation, through the receiver: its minutes decode, on time.
#[test]
fn tdf_decodes_from_the_phase() {
    let st = Station::Tdf;
    let start = days_from_civil(2026, 11, 16) * 86_400 + 10 * 3600 + 20 * 60;
    let lead = 23.4;
    let minutes = 5;
    let syms: Vec<char> = (0..minutes)
        .flat_map(|m| web888_timecode::encode(st, start + 60 * m as i64))
        .collect();
    let mut rng = Rng(11);
    let n = ((minutes as f64 * 60.0 - lead - 1.0) * FS) as usize;
    let mut busy = 0x9e37_79b9_7f4a_7c15u64;
    let x: Vec<f32> = (0..n)
        .map(|i| {
            let tt = lead + i as f64 / FS;
            let s = tt as usize;
            if i % FS as usize == 0 {
                busy = busy.rotate_left(7) ^ 0x2545_f491_4f6c_dd1d;
            }
            let ph = tdf_phase(syms[s], syms[s + 1], tt - s as f64, busy);
            ((2.0 * std::f64::consts::PI * 1000.0 * (i as f64 / FS) + ph).sin()
                + 0.1 * rng.normal()) as f32
        })
        .collect();
    let mut rx = Receiver::new(st, FS, 1000.0, 0.0);
    let mut good = 0;
    for c in x.chunks(1200) {
        rx.push(c);
        for e in rx.poll() {
            if let Event::Minute(m) = e {
                let want = (m.utc - start) as f64 - lead;
                assert!(
                    (m.t - want).abs() < 0.03,
                    "minute at {} s, wanted {want}",
                    m.t
                );
                good += m.ok as usize;
            }
        }
    }
    assert!(good >= 3, "{good} minutes");
}

/// "auto" picks what is in reach: all of Europe's in Frankfurt, JJY in
/// Tokyo, WWVB in Denver, nothing in Sydney; regions by name.
#[test]
fn stations_by_region() {
    let names = |v: Vec<Station>| v.iter().map(|s| s.name()).collect::<Vec<_>>().join(",");
    let auto = |p| names(Station::for_region("auto", Some(p)).unwrap());
    assert_eq!(auto((50.11, 8.68)), "DCF77,MSF,RBU,DCF39,HGA22,DCF49,TDF");
    assert_eq!(auto((35.68, 139.69)), "JJY,BPC");
    assert_eq!(auto((39.74, -104.99)), "WWVB");
    assert_eq!(auto((-33.87, 151.21)), "");
    assert_eq!(Station::for_region("auto", None), None);
    assert_eq!(
        names(Station::for_region("east-asia", None).unwrap()),
        "JJY,BPC"
    );
    assert_eq!(Station::for_region("mars", None), None);
    assert!(!names(Station::for_region("all", None).unwrap()).contains("WWV/"));
}

/// BPM and RWM: ticks only. Their minutes pass on the minute's mark and
/// the ticks, unnamed (no time code), confirmed 60 s apart; at the
/// minute's start.
#[test]
fn ticks_find_the_minute() {
    for st in [Station::Bpm, Station::Rwm] {
        ticks(st);
    }
}

fn ticks(st: Station) {
    let start = days_from_civil(2026, 11, 16) * 86_400 + 10 * 3600 + 20 * 60;
    let lead = 23.4;
    let x = audio(st, start, 5, 1000.0, lead, 0.3, 7);
    let mut rx = Receiver::new(st, FS, 1000.0, 0.0);
    let mut good = 0;
    for c in x.chunks(1200) {
        rx.push(c);
        for e in rx.poll() {
            if let Event::Minute(m) = e {
                assert!(!m.named);
                if m.ok && m.confirmed {
                    good += 1;
                    // a minute's start: a whole minute from the stream's
                    // start less the lead
                    let k = ((m.t + lead) / 60.0).round();
                    assert!((m.t + lead - 60.0 * k).abs() < 0.003, "minute at {} s", m.t);
                }
            }
        }
    }
    assert!(good >= 2, "{st:?}: {good} confirmed");
}
