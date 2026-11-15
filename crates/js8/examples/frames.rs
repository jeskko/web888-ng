// SPDX-License-Identifier: GPL-3.0-or-later
//! Frames in a recording: `frames AUDIO.f32 START [SUBMODES]`. AUDIO is
//! 12 kS/s mono f32 (USB), START its first sample's UTC time of day
//! (`HH:MM:SS[.s]`), SUBMODES e.g. `normal,fast` (default all). Each
//! period of each submode is decoded; prints UTC, submode, SNR, dt,
//! audio Hz, frame type and the 12 characters.
use std::io::Read;
use web888_js8::{Frames, Submode};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut bytes = Vec::new();
    std::fs::File::open(&a[1])
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    let audio: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    let hms: Vec<f64> = a[2].split(':').map(|v| v.parse().unwrap()).collect();
    let start = hms[0] * 3600.0 + hms[1] * 60.0 + hms[2];
    let modes: Vec<Submode> = match a.get(3) {
        Some(s) => s
            .split(',')
            .map(|n| *Submode::ALL.iter().find(|m| m.name() == n).unwrap())
            .collect(),
        None => Submode::ALL.to_vec(),
    };
    let mut frames = Frames::new();
    let mut total = 0;
    let t = std::time::Instant::now();
    for sm in modes {
        let p = sm.period() as f64;
        let mut s = (start / p).ceil() * p;
        let mut n = 0;
        while ((s - start) * 12000.0) as usize + (p * 12000.0) as usize <= audio.len() {
            let i = ((s - start) * 12000.0).round() as usize;
            let len = ((p * 12000.0) as usize).min(audio.len() - i);
            for f in frames.decode(sm, &audio[i..i + len]) {
                let hh = (s / 3600.0) as u32 % 24;
                let mm = (s / 60.0) as u32 % 60;
                let ss = s as u32 % 60;
                let text = web888_js8::message::unpack(f.bits >> 3, f.kind)
                    .map_or("(unreadable)".to_string(), |u| u.text);
                println!(
                    "{hh:02}{mm:02}{ss:02} {:6} {:+4.0} {:+5.2} {:6.1}  {} {}  {}",
                    sm.name(),
                    f.snr,
                    f.dt,
                    f.freq,
                    f.kind,
                    f.text,
                    text.replace('\n', " | ")
                );
                n += 1;
            }
            s += p;
        }
        eprintln!("{}: {n} frames", sm.name());
        total += n;
    }
    eprintln!("{total} frames, {:.1} s", t.elapsed().as_secs_f64());
}
