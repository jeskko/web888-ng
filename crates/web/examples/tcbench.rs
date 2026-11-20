// SPDX-License-Identifier: GPL-3.0-or-later
//! The time-code monitor's cost and output on a recorded bin: a SigMF
//! `cf32_le` recording of one 60 kS/s channelizer bin (`web888-client
//! wide`), fed in runs as small as the unit's; prints each report as a
//! JSON line and, to stderr, the CPU time per second of signal.
//!
//!   cargo run --release -p web888-web --example tcbench -- \
//!       FILE.sigmf-data START_UTC [--centre 60000] [--secs N] \
//!       [--stations MSF,RBU,DCF77] [--run 64]
//!
//! `-` reads the samples from stdin (on the unit: streamed over ssh). All
//! of it is read first (60 s: ~58 MB), then timed as a whole.

use std::io::Read;
use web888_radio::Iq32;
use web888_radio::chan::RX_SINE_FULL_SCALE;
use web888_web::tcmon::Monitor;

fn cpu_s() -> f64 {
    let mut t = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: plain clock read into a local.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
    t.tv_sec as f64 + t.tv_nsec as f64 * 1e-9
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let path = a.get(1).expect("FILE.sigmf-data or -");
    let start: f64 = a
        .get(2)
        .and_then(|s| s.parse().ok())
        .expect("START_UTC (s)");
    let opt = |k: &str| a.iter().position(|x| x == k).and_then(|i| a.get(i + 1));
    let centre: f64 = opt("--centre").map_or(60_000.0, |s| s.parse().unwrap());
    let secs: f64 = opt("--secs").map_or(f64::INFINITY, |s| s.parse().unwrap());
    let run: usize = opt("--run").map_or(64, |s| s.parse().unwrap());
    let stations: Vec<String> = opt("--stations")
        .map_or("MSF,RBU,DCF77", |s| s.as_str())
        .split(',')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    let rate = 60_000.0;
    let mut mon = Monitor::new(&stations, centre, rate).expect("monitor");
    let input: Box<dyn Read> = if path == "-" {
        Box::new(std::io::stdin().lock())
    } else {
        Box::new(std::fs::File::open(path).expect("open"))
    };
    // all of it in memory first: the timing is the monitor's alone
    let mut all: Vec<Iq32> = Vec::new();
    let mut bytes = Vec::new();
    input
        .take((secs * rate * 8.0).min(u64::MAX as f64) as u64)
        .read_to_end(&mut bytes)
        .expect("read");
    all.extend(bytes.as_chunks::<8>().0.iter().map(|b| {
        let i = f32::from_le_bytes(b[..4].try_into().unwrap()) as f64;
        let q = f32::from_le_bytes(b[4..].try_into().unwrap()) as f64;
        Iq32 {
            i: (i * RX_SINE_FULL_SCALE).round() as i32,
            q: (q * RX_SINE_FULL_SCALE).round() as i32,
        }
    }));
    drop(bytes);
    all.truncate((secs * rate).min(all.len() as f64) as usize);
    let utc = |f: f64| Some(start + f / rate);
    let mut out = Vec::new();
    let t0 = cpu_s();
    for (k, iq) in all.chunks(run).enumerate() {
        out.extend(mon.push((k * run) as u64, iq, utc, None));
    }
    let cpu = cpu_s() - t0;
    for r in &out {
        println!("{}", serde_json::to_string(r).unwrap());
    }
    let first = all.len();
    let s = first as f64 / rate;
    eprintln!(
        "{s:.0} s of signal: {cpu:.3} s CPU, {:.2} % of a core",
        cpu / s * 100.0
    );
}
