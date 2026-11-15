// SPDX-License-Identifier: GPL-3.0-or-later
//! Decodes HFDL from a complex f32 file (I, Q interleaved), or from audio
//! (`AUDIO=1`: mono f32).
//!
//! `replay FILE RATE [CENTRE]`: CENTRE (Hz, default 0, audio 1440) is the
//! burst's centre in the file (the listed frequency + 1440 Hz). Prints
//! each burst (time, rate, frequency offset, SNR) and its parts as the
//! web UI shows them. `P=mu_known,mu_dd,pll,pll_i,ridge[,pll_dd[,interp]]`
//! sets the receiver's constants.
use num_complex::Complex32;
use std::io::Read;
use web888_hfdl::skim::Decoder;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let rate: f64 = a[2].parse().unwrap();
    let audio = std::env::var("AUDIO").is_ok();
    let centre: f64 = a
        .get(3)
        .map_or(if audio { 1440.0 } else { 0.0 }, |s| s.parse().unwrap());
    let mut f = std::fs::File::open(&a[1]).unwrap();
    let mut dec = Decoder::new(rate, centre);
    if let Ok(v) = std::env::var("P") {
        let v: Vec<f64> = v.split(',').map(|x| x.parse().unwrap()).collect();
        let p = dec.params();
        (p.mu_known, p.mu_dd, p.pll, p.pll_i, p.ridge) =
            (v[0] as f32, v[1] as f32, v[2] as f32, v[3] as f32, v[4]);
        p.pll_dd = v.get(5).is_some_and(|&x| x != 0.0);
        p.interp = v.get(6).is_none_or(|&x| x != 0.0);
    }
    let mut buf = vec![0u8; 8 * 8192];
    loop {
        let mut got = 0;
        while got < buf.len() {
            let n = f.read(&mut buf[got..]).unwrap();
            if n == 0 {
                break;
            }
            got += n;
        }
        let x: Vec<Complex32> = if audio {
            buf[..got / 4 * 4]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| Complex32::new(f32::from_le_bytes(*c), 0.0))
                .collect()
        } else {
            buf[..got / 8 * 8]
                .as_chunks::<8>()
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
        if x.is_empty() {
            break;
        }
        for m in dec.push(&x) {
            let gs = m.gs.map_or(String::new(), |g| {
                web888_hfdl::stations::station(g).map_or(format!("GS {g}"), |s| {
                    web888_hfdl::skim::short(s.name).to_string()
                })
            });
            println!(
                "{:7.2} {:4} bps {} f {:+5.1} snr {:5.1}  {} {}",
                m.t,
                m.bps,
                if m.double { 'D' } else { 'S' },
                m.freq,
                m.snr,
                if m.dir.is_empty() {
                    "(CRC failed)"
                } else {
                    m.dir
                },
                gs
            );
            for it in &m.items {
                let who = [it.flight.as_str(), it.reg.as_str()]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(" ");
                let icao = it.icao.map_or(String::new(), |i| format!(" {i:06X}"));
                let pos = it
                    .pos
                    .map_or(String::new(), |(a, o)| format!(" @{a:.3},{o:.3}"));
                let ac = it.ac.map_or(String::new(), |a| format!("AC {a:3} "));
                println!(
                    "    {ac}[{}] {who}{icao}{pos}: {}",
                    it.kind,
                    it.text.replace('\n', " | ")
                );
            }
        }
    }
    eprintln!(
        "{} bursts, {} decoded, {} false starts, {:.0} s",
        dec.bursts,
        dec.decoded,
        dec.false_starts(),
        dec.now()
    );
}
