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
