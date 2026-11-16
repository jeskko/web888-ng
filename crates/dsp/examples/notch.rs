// SPDX-License-Identifier: GPL-3.0-or-later
//! The automatic notch on a recording: 16-bit mono WAV at 12 kS/s in,
//! what it took out (dB, and the share of the time it notched anything),
//! and the result as WAV.
//!
//!     cargo run --release -p web888-dsp --example notch IN.wav OUT.wav

use web888_dsp::notch::AutoNotch;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let b = std::fs::read(&a[1]).expect("read");
    // the samples: the "data" chunk of a plain PCM file
    let mut p = 12;
    let data = loop {
        let id = &b[p..p + 4];
        let len = u32::from_le_bytes(b[p + 4..p + 8].try_into().unwrap()) as usize;
        if id == b"data" {
            break &b[p + 8..(p + 8 + len).min(b.len())];
        }
        p += 8 + len + (len & 1);
    };
    let x: Vec<f32> = data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| i16::from_le_bytes(*c) as f32 / 32768.0)
        .collect();
    let mut y = x.clone();
    let mut an = AutoNotch::new();
    let hop = 256;
    let mut busy = 0;
    for c in y.chunks_mut(hop) {
        an.process(c);
        busy += usize::from(an.active());
    }
    let d = AutoNotch::DELAY;
    let p = |v: &[f32]| v.iter().map(|s| s * s).sum::<f32>();
    let (pin, pout) = (p(&x[..x.len() - d]), p(&y[d..]));
    println!(
        "{}: {:+.2} dB, notching {:.0} % of the time",
        a[1],
        10.0 * (pout / pin).log10(),
        100.0 * busy as f64 / x.len().div_ceil(hop) as f64
    );
    if let Some(out) = a.get(2) {
        let mut w = b[..44].to_vec();
        w[40..44].copy_from_slice(&((y.len() * 2) as u32).to_le_bytes());
        w[4..8].copy_from_slice(&((36 + y.len() * 2) as u32).to_le_bytes());
        w[12..16].copy_from_slice(b"fmt ");
        w[16..20].copy_from_slice(&16u32.to_le_bytes());
        w[20..22].copy_from_slice(&1u16.to_le_bytes());
        w[22..24].copy_from_slice(&1u16.to_le_bytes());
        w[24..28].copy_from_slice(&12_000u32.to_le_bytes());
        w[28..32].copy_from_slice(&24_000u32.to_le_bytes());
        w[32..34].copy_from_slice(&2u16.to_le_bytes());
        w[34..36].copy_from_slice(&16u16.to_le_bytes());
        w[36..40].copy_from_slice(b"data");
        for s in &y {
            w.extend(((s * 32768.0).clamp(-32768.0, 32767.0) as i16).to_le_bytes());
        }
        std::fs::write(out, w).expect("write");
    }
}
