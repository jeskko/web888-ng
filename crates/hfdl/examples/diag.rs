// SPDX-License-Identifier: GPL-3.0-or-later
//! How the burst format was checked on air (`/data/iq/hfdl/README.md`).
//! `diag FILE RATE [CENTRE]` (complex f32) with one of:
//!
//! * (none): each strong 300 bps burst's coded-bit pairs (each bit is sent
//!   twice) agreeing after descrambling and deinterleaving, for scrambler
//!   variants: 100 % with the right one.
//! * `PERIOD=1`: the raw bits' agreement at several lags.
//! * `SIM=1`: how alike the strong bursts' raw bits are.
//! * `COMMON=1`: the bits all strong bursts share (zero padding: the
//!   scrambler itself) folded mod 120.
//! * `CODE=1`: a strong burst Viterbi decoded and re-encoded with each
//!   ordering of the generators.
//! * `REF=t`: the burst decoded at t re-encoded, and every burst of its
//!   type's symbol errors against it (per 6 frames).
//! * `GAINS=t` (`FROZEN=1`: the equaliser not adapted): each probe's gain.
//! * `PEAKS=1`: the detection metric's local maxima.
use num_complex::Complex32;
use std::io::Read;
use web888_hfdl::demod::Demod;
use web888_hfdl::front::Front;
use web888_hfdl::phy::{self, Mode};

fn scramblers() -> Vec<(&'static str, Vec<u8>)> {
    let mut v = vec![
        ("none", vec![0u8; 120]),
        ("fib 0x4001/0x6959", phy::scrambler().to_vec()),
    ];
    // Galois, right shift
    for (name, g, a) in [
        ("gal 0x4001/0x4d4b", 0x4001u32, 0x4d4bu32),
        ("gal 0x6000/0x4d4b", 0x6000, 0x4d4b),
        ("gal 0x4001/0x6959", 0x4001, 0x6959),
    ] {
        let mut s = a;
        let seq = (0..120)
            .map(|_| {
                let b = s & 1;
                s >>= 1;
                if b == 1 {
                    s ^= g;
                }
                b as u8
            })
            .collect();
        v.push((name, seq));
    }
    v
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let rate: f64 = a[2].parse().unwrap();
    let centre: f64 = a.get(3).map_or(0.0, |s| s.parse().unwrap());
    let mut bytes = Vec::new();
    std::fs::File::open(&a[1])
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    let x: Vec<Complex32> = bytes
        .as_chunks::<8>()
        .0
        .iter()
        .map(|c| {
            Complex32::new(
                f32::from_le_bytes(c[..4].try_into().unwrap()),
                f32::from_le_bytes(c[4..].try_into().unwrap()),
            )
        })
        .collect();
    let mut front = Front::new(rate, centre);
    let mut y = Vec::new();
    front.push(&x, &mut y);
    let mut demod = Demod::new();
    if std::env::var("FROZEN").is_ok() {
        demod.params.mu_known = 0.0;
        demod.params.mu_dd = 0.0;
        demod.params.pll = 0.0;
        demod.params.pll_i = 0.0;
    }
    let show_gains = std::env::var("GAINS")
        .ok()
        .map(|v| v.parse::<f64>().unwrap());
    demod.trace = std::env::var("PEAKS").is_ok();
    let bursts = demod.push(&y);
    if demod.trace {
        // local maxima
        let p = &demod.peaks;
        for i in 0..p.len() {
            let (n, m) = p[i];
            let lo = p[..i]
                .iter()
                .rev()
                .take_while(|q| n - q.0 < 400)
                .all(|q| q.1 <= m);
            let hi = p[i + 1..]
                .iter()
                .take_while(|q| q.0 - n < 400)
                .all(|q| q.1 < m);
            if lo && hi {
                println!("{:8.3} {:.2}", front.time(n) - 448.0 / 1800.0, m);
            }
        }
        return;
    }
    if let Some(t) = show_gains {
        let b = bursts
            .iter()
            .find(|b| (front.time(b.start) - 448.0 / 1800.0 - t).abs() < 0.5)
            .unwrap();
        for (j, g) in b.gains.iter().enumerate() {
            println!(
                "{j:3} |g| {:.2} arg {:+4.0}",
                g.norm(),
                g.arg().to_degrees()
            );
        }
        return;
    }
    let good: Vec<_> = bursts
        .iter()
        .filter(|b| b.snr > 8.0 && b.mode.repeat)
        .collect();
    if let Ok(r) = std::env::var("REF") {
        // REF=t: the burst decoded near t, re-encoded, against the others
        // of its mode: symbol errors per 6 frames
        let t: f64 = r.parse().unwrap();
        let rb = bursts
            .iter()
            .find(|b| (front.time(b.start) - 448.0 / 1800.0 - t).abs() < 0.5)
            .unwrap();
        let sym = web888_hfdl::synth::symbols(rb.mode, &rb.octets, &mut web888_hfdl::synth::Rng(1));
        let data: Vec<Complex32> = sym[448 + phy::PREAMBLE..]
            .chunks(45)
            .flat_map(|c| c[..30].to_vec())
            .collect();
        for b in bursts.iter().filter(|b| b.mode == rb.mode) {
            let errs: Vec<usize> = b
                .symbols
                .chunks(180)
                .zip(data.chunks(180))
                .map(|(z, d)| {
                    z.iter()
                        .zip(d)
                        .filter(|(z, d)| (z.re < 0.0) != (d.re < 0.0))
                        .count()
                })
                .collect();
            let ok = web888_hfdl::pdu::parse(&b.octets).is_some();
            println!(
                "{:7.2} snr {:4.1} {} {:?}",
                front.time(b.start) - 448.0 / 1800.0,
                b.snr,
                if ok { "ok  " } else { "FAIL" },
                errs
            );
        }
        return;
    }
    if std::env::var("CODE").is_ok() {
        let b = good[0];
        let llr = web888_hfdl::demod::soft_bits(&b.symbols, &b.gains, b.mode, b.sigma2);
        let perm = phy::interleave(&b.mode);
        let de: Vec<f32> = perm.iter().map(|&p| llr[p as usize]).collect();
        let de: Vec<f32> = de.chunks(2).map(|c| c[0] + c[1]).collect();
        for (pa, pb) in [(0x4f, 0x6d), (0x6d, 0x4f), (0x79, 0x5b), (0x5b, 0x79)] {
            for inv in [false, true] {
                let l: Vec<f32> = de.iter().map(|v| if inv { -v } else { *v }).collect();
                let bits = viterbi_with(&l, [pa, pb]);
                let re = encode_with(&bits, [pa, pb]);
                let agree = re
                    .iter()
                    .zip(&l)
                    .filter(|(c, v)| (**c == 0) == (**v > 0.0))
                    .count();
                println!("{pa:02x}/{pb:02x} inv {inv}: {agree}/{}", l.len());
            }
        }
        return;
    }
    if std::env::var("COMMON").is_ok() {
        let hs: Vec<Vec<u8>> = good
            .iter()
            .map(|b| b.symbols.iter().map(|z| (z.re < 0.0) as u8).collect())
            .collect();
        let n = hs[0].len();
        let common: Vec<Option<u8>> = (0..n)
            .map(|i| hs.iter().all(|h| h[i] == hs[0][i]).then_some(hs[0][i]))
            .collect();
        println!("common {} of {n}", common.iter().flatten().count());
        let mut seq = String::new();
        let (mut pure, mut tot) = (0, 0);
        for r in 0..120 {
            let v: Vec<u8> = (r..n).step_by(120).filter_map(|i| common[i]).collect();
            let ones = v.iter().filter(|&&b| b == 1).count();
            tot += v.len();
            pure += ones.max(v.len() - ones);
            seq.push(if v.is_empty() {
                '?'
            } else if ones * 2 > v.len() {
                '1'
            } else {
                '0'
            });
        }
        println!("purity {:.3} ({tot})\n  {seq}", pure as f64 / tot as f64);
        let s: String = phy::scrambler()
            .iter()
            .map(|&b| (b'0' + b) as char)
            .collect();
        println!("  {s}");
        return;
    }
    if std::env::var("SIM").is_ok() {
        let hs: Vec<Vec<u8>> = good
            .iter()
            .map(|b| b.symbols.iter().map(|z| (z.re < 0.0) as u8).collect())
            .collect();
        for (i, a) in hs.iter().enumerate() {
            let row: Vec<String> = hs
                .iter()
                .map(|b| {
                    format!(
                        "{:.2}",
                        a.iter().zip(b).filter(|(x, y)| x == y).count() as f64 / a.len() as f64
                    )
                })
                .collect();
            println!("{:7.1} {}", front.time(good[i].start), row.join(" "));
        }
        return;
    }
    for b in good {
        let m: Mode = b.mode;
        let perm = phy::interleave(&m);
        let h0: Vec<u8> = b.symbols.iter().map(|z| (z.re < 0.0) as u8).collect();
        if std::env::var("PERIOD").is_ok() {
            for lag in [1, 15, 30, 60, 119, 120, 121, 240] {
                print!(" lag {lag}: {:.2}", period(&h0, lag));
            }
            println!();
            let s: String = h0[..120].iter().map(|&b| (b'0' + b) as char).collect();
            println!("  h[0..120]   {s}");
            let s: String = h0[120..240].iter().map(|&b| (b'0' + b) as char).collect();
            println!("  h[120..240] {s}");
            let s: String = phy::scrambler()
                .iter()
                .map(|&b| (b'0' + b) as char)
                .collect();
            println!("  ours        {s}");
            continue;
        }
        print!("snr {:4.1}:", b.snr);
        for (name, s) in scramblers() {
            let h: Vec<u8> = b
                .symbols
                .iter()
                .enumerate()
                .map(|(i, z)| (z.re < 0.0) as u8 ^ s[i % 120])
                .collect();
            let de: Vec<u8> = perm.iter().map(|&p| h[p as usize]).collect();
            let agree =
                de.chunks(2).filter(|c| c[0] == c[1]).count() as f64 / (de.len() / 2) as f64;
            // without the interleaver: neighbours
            let nb = h.chunks(2).filter(|c| c[0] == c[1]).count() as f64 / (h.len() / 2) as f64;
            print!("  {name}: {:.2} ({:.2})", agree, nb);
        }
        println!();
    }
}

/// Hard bits' agreement with themselves `lag` symbols on, and the first
/// 120 hard bits.
pub fn period(h: &[u8], lag: usize) -> f64 {
    h.iter().zip(&h[lag..]).filter(|(a, b)| a == b).count() as f64 / (h.len() - lag) as f64
}

fn encode_with(bits: &[u8], poly: [u32; 2]) -> Vec<u8> {
    let mut sr = 0u32;
    let mut out = Vec::new();
    for &b in bits {
        sr = ((sr << 1) | b as u32) & 0x7f;
        for p in poly {
            out.push(((sr & p).count_ones() & 1) as u8);
        }
    }
    out
}

fn viterbi_with(llr: &[f32], poly: [u32; 2]) -> Vec<u8> {
    let n = llr.len() / 2;
    let out: Vec<[u8; 2]> = (0..128u32)
        .map(|r| poly.map(|p| ((r & p).count_ones() & 1) as u8))
        .collect();
    let mut metric = [f32::NEG_INFINITY; 64];
    metric[0] = 0.0;
    let mut from = vec![[0u8; 64]; n];
    for k in 0..n {
        let mut next = [f32::NEG_INFINITY; 64];
        for s in 0..64 {
            for old in [s >> 1, (s >> 1) | 32] {
                let o = out[(old << 1) | (s & 1)];
                let m = metric[old]
                    + if o[0] == 0 { llr[2 * k] } else { -llr[2 * k] }
                    + if o[1] == 0 {
                        llr[2 * k + 1]
                    } else {
                        -llr[2 * k + 1]
                    };
                if m > next[s] {
                    next[s] = m;
                    from[k][s] = old as u8;
                }
            }
        }
        metric = next;
    }
    let mut s = (0..64)
        .max_by(|&a, &b| metric[a].total_cmp(&metric[b]))
        .unwrap();
    let mut bits = vec![0u8; n];
    for k in (0..n).rev() {
        bits[k] = (s & 1) as u8;
        s = from[k][s] as usize;
    }
    bits
}
