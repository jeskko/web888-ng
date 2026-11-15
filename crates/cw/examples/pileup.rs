// SPDX-License-Identifier: GPL-3.0-or-later
//! Pile-ups: how many callers' calls the skimmer reads. A running station
//! at 0 Hz sends "TU TEST <call>" every round; a pool of 2-4 callers
//! (22-35 WPM, within ±6 dB of each other) answers each round, each
//! caller with probability 0.8, starting 0-0.8 s after the runner stops,
//! sending its call once or twice; after each round one caller is worked
//! and replaced by a new one (the runner's exchange leaves the call out:
//! only the callers' own sending counts). The callers' spread around the runner:
//! tight (±40 Hz), medium (±100 Hz) or wide (±300 Hz). Every station
//! fades on its own (`FADE`): `none`; `qsb` (the default: Rayleigh, 0.05-0.5
//! Hz Doppler); `aurora` (flutter: a Gaussian Doppler spectrum, 3-20 Hz
//! spread).
//!
//! Scored per spread: the share of callers whose call was read as a word
//! of its own at least once, and the callsign-like words (a letter and a
//! digit, 4-10 characters) that are no station's call (garbage: callers
//! run together or misread).
//!
//!     [FADE=none|qsb|aurora] [SPACING=hz] cargo run --release -p web888-cw --example pileup [neural 0|1] [trials]

use num_complex::Complex32;
use std::collections::{BTreeMap, HashSet};
use web888_cw::Skimmer;
use web888_cw::dsp::Rng;
use web888_cw::synth::{self, Fading, Keying};

const RATE: f64 = 12_000.0;
const SECONDS: f64 = 60.0;

fn callsign(r: &mut Rng) -> String {
    let l = |r: &mut Rng| (b'A' + (r.uniform() * 26.0) as u8) as char;
    let d = |r: &mut Rng| (b'0' + (r.uniform() * 10.0) as u8) as char;
    let mut s = String::new();
    match (r.uniform() * 3.0) as u32 {
        0 => s.push(l(r)),
        1 => {
            s.push(l(r));
            s.push(l(r));
        }
        _ => {
            s.push(l(r));
            s.push(d(r));
        }
    }
    s.push(d(r));
    for _ in 0..[1, 2, 2, 3, 3][(r.uniform() * 5.0) as usize] {
        s.push(l(r));
    }
    s
}

struct Caller {
    fade: Option<Fading>,
    call: String,
    hz: f64,
    amp: f32,
    wpm: f64,
}

/// One trial: the IQ and every call sent (the runner's included).
/// A station's fading (Doppler range in Hz, Gaussian; None: none).
fn fader(r: &mut Rng, doppler: Option<(f64, f64, bool)>) -> Option<Fading> {
    doppler.map(|(a, b, gauss)| {
        let hz = a + (b - a) * r.uniform();
        if gauss {
            Fading::gaussian(hz, r)
        } else {
            Fading::new(hz, r)
        }
    })
}

fn scenario(
    r: &mut Rng,
    spread: f64,
    doppler: Option<(f64, f64, bool)>,
) -> (Vec<Complex32>, Vec<(String, f64)>, HashSet<String>) {
    // 8 s of noise after the last round: what was sent is all printed
    let n = ((SECONDS + 8.0) * RATE) as usize;
    let sd = 0.01f32;
    let mut x: Vec<Complex32> = (0..n)
        .map(|_| Complex32::new(sd * r.normal() as f32, sd * r.normal() as f32))
        .collect();
    // amplitude for an SNR in 2500 Hz: amp² / (2 sd² 2500 / rate)
    let amp_of = |snr: f64| {
        (2.0 * (sd as f64).powi(2) * 2500.0 / RATE * 10f64.powf(snr / 10.0)).sqrt() as f32
    };
    let add = |x: &mut Vec<Complex32>,
               r: &mut Rng,
               text: &str,
               at: f64,
               hz: f64,
               amp: f32,
               wpm: f64,
               fade: &Option<Fading>|
     -> f64 {
        let k = Keying {
            lead: 0.0,
            tail: 0.0,
            jitter: 0.04,
            ..Keying::new(wpm)
        };
        let key = synth::keying(text, &k, RATE, r);
        let i0 = (at * RATE) as usize;
        let ph0 = r.uniform() * std::f64::consts::TAU;
        for (i, &kv) in key.iter().enumerate() {
            if i0 + i >= x.len() {
                break;
            }
            if kv == 0.0 {
                continue;
            }
            let t = (i0 + i) as f64 / RATE;
            let p = std::f64::consts::TAU * hz * t + ph0;
            let mut v = Complex32::new(amp * kv * p.cos() as f32, amp * kv * p.sin() as f32);
            if let Some(f) = fade {
                let (gr, gi) = f.gain(t);
                v *= Complex32::new(gr as f32, gi as f32);
            }
            x[i0 + i] += v;
        }
        at + key.len() as f64 / RATE
    };
    let run_call = callsign(r);
    let run_amp = amp_of(10.0 + 10.0 * r.uniform());
    let run_fade = fader(r, doppler);
    let caller_snr = 6.0 + 12.0 * r.uniform();
    let new_caller = |r: &mut Rng| Caller {
        fade: fader(r, doppler),
        call: callsign(r),
        hz: spread * (2.0 * r.uniform() - 1.0),
        amp: amp_of(caller_snr + 12.0 * r.uniform() - 6.0),
        wpm: 22.0 + 13.0 * r.uniform(),
    };
    let mut pool: Vec<Caller> = (0..2 + (r.uniform() * 3.0) as usize)
        .map(|_| new_caller(r))
        .collect();
    let mut sent = Vec::new();
    let mut all = HashSet::from([run_call.clone()]);
    let mut t = 1.0;
    while t < SECONDS - 8.0 {
        t = add(
            &mut x,
            r,
            &format!("TU TEST {run_call}"),
            t,
            0.0,
            run_amp,
            30.0,
            &run_fade,
        );
        let mut end = t;
        for c in &pool {
            if r.uniform() < 0.8 {
                let text = if r.uniform() < 0.5 {
                    c.call.clone()
                } else {
                    format!("{} {}", c.call, c.call)
                };
                let at = t + 0.8 * r.uniform();
                let e = add(&mut x, r, &text, at, c.hz, c.amp, c.wpm, &c.fade);
                end = end.max(e);
                sent.push((c.call.clone(), c.hz));
                all.insert(c.call.clone());
            }
        }
        t = end + 0.5 + 0.5 * r.uniform();
        // one worked: the runner's exchange, the caller's, and a new caller
        let w = (r.uniform() * pool.len() as f64) as usize;
        let c = pool.remove(w);
        t = add(&mut x, r, "R 5NN 14", t, 0.0, run_amp, 30.0, &run_fade);
        t = add(&mut x, r, "TU 5NN 15", t + 0.4, c.hz, c.amp, c.wpm, &c.fade) + 0.4;
        sent.push((c.call.clone(), c.hz));
        pool.push(new_caller(r));
    }
    sent.sort_by(|a, b| a.0.cmp(&b.0));
    sent.dedup_by(|a, b| a.0 == b.0);
    (x, sent, all)
}

fn callish(w: &str) -> bool {
    (4..=10).contains(&w.len())
        && w.chars().all(|c| c.is_ascii_alphanumeric())
        && w.chars().any(|c| c.is_ascii_digit())
        && w.chars().any(|c| c.is_ascii_alphabetic())
        && !w.starts_with("5NN")
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let neural = a.get(1).is_none_or(|v| v == "1");
    let trials: u64 = a.get(2).and_then(|v| v.parse().ok()).unwrap_or(12);
    let verbose = std::env::var("VERBOSE").is_ok();
    let fade = std::env::var("FADE").unwrap_or_else(|_| "qsb".into());
    let doppler = match fade.as_str() {
        "none" => None,
        "aurora" => Some((3.0, 20.0, true)),
        _ => Some((0.05, 0.5, false)),
    };
    println!("fading: {fade}");
    for (name, spread) in [
        ("tight ±40 Hz", 40.0),
        ("medium ±100 Hz", 100.0),
        ("wide ±300 Hz", 300.0),
    ] {
        let (mut calls, mut read, mut words, mut garbage) = (0, 0, 0, 0);
        for trial in 0..trials {
            let mut r = Rng::new(100 + trial);
            let (x, sent, all) = scenario(&mut r, spread, doppler);
            let mut sk = Skimmer::new_iq(RATE, -1000.0, 1000.0);
            sk.neural = neural;
            if let Some(v) = std::env::var("HI_PCT").ok().and_then(|v| v.parse().ok()) {
                sk.hi_pct = v;
            }
            if let Some(v) = std::env::var("SPACING").ok().and_then(|v| v.parse().ok()) {
                sk.spacing = v;
            }
            let mut text = BTreeMap::<u32, (f64, String)>::new();
            for c in x.chunks(2048) {
                sk.push_iq(c);
                for e in sk.poll() {
                    let t = text.entry(e.id).or_default();
                    t.0 = e.hz;
                    t.1.push_str(&e.text);
                }
            }
            let got: HashSet<&str> = text.values().flat_map(|t| t.1.split(' ')).collect();
            calls += sent.len();
            read += sent.iter().filter(|c| got.contains(c.0.as_str())).count();
            for w in text
                .values()
                .flat_map(|t| t.1.split(' '))
                .filter(|w| callish(w))
            {
                words += 1;
                garbage += usize::from(!all.contains(w));
            }
            if verbose {
                println!("-- {name} trial {trial}");
                for (c, hz) in &sent {
                    println!(
                        "  {hz:>5.0} {c}{}",
                        if got.contains(c.as_str()) {
                            ""
                        } else {
                            "  MISSED"
                        }
                    );
                }
                for (hz, t) in text.values() {
                    println!("{hz:>7.0} {t}");
                }
            }
        }
        println!(
            "{name:<15} calls read {read}/{calls} ({:.0} %), callsign-like words that are no call {garbage}/{words} ({:.0} %)",
            100.0 * read as f64 / calls.max(1) as f64,
            100.0 * garbage as f64 / words.max(1) as f64
        );
    }
}
