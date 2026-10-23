// SPDX-License-Identifier: GPL-3.0-or-later
//! Per-symbol look at a known FT8 signal in a WAV: power on the true tone
//! against the strongest other tone, from plain 1920-sample FFTs.
//!
//!   ft8probe FILE.wav FREQ DT "MESSAGE"

use rustfft::{FftPlanner, num_complex::Complex32 as C32};
use web888_ftx::{crc, ft8, ldpc, msg, osd, tones, wav};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (x, _) = wav::read(a[1].as_ref()).unwrap();
    let f0: f32 = a[2].parse().unwrap();
    let dt: f32 = a[3].parse().unwrap();
    let m = msg::pack(&a[4]).expect("packable message");
    let t = tones::ft8(m);
    let verbose = a.get(5).is_some_and(|v| v == "-v");
    // 4x zero padding: bins of 1.5625 Hz, tone k at bin 4k from f0.
    let n = 4 * ft8::NSPS;
    let fft = FftPlanner::new().plan_fft_forward(n);
    let start = ((dt + ft8::T0) * ft8::RATE).round() as i64;
    let b0 = (f0 / (ft8::RATE / n as f32)).round() as usize;
    for (k, &tone) in t.iter().enumerate() {
        let s = start + (k * ft8::NSPS) as i64;
        let mut buf: Vec<C32> = (0..n)
            .map(|i| {
                let j = s + i as i64;
                let v = if i < ft8::NSPS && j >= 0 && (j as usize) < x.len() {
                    x[j as usize]
                } else {
                    0.0
                };
                C32::new(v, 0.0)
            })
            .collect();
        fft.process(&mut buf);
        let p: Vec<f32> = (0..8).map(|tt| buf[b0 + 4 * tt].norm_sqr()).collect();
        let other = (0..8)
            .filter(|&tt| tt != tone as usize)
            .map(|tt| p[tt])
            .fold(0.0, f32::max);
        // Strongest bin anywhere from -20 to +70 Hz, at 1.5625 Hz.
        let (bi, bp) = (0..58)
            .map(|i| (i - 13, buf[(b0 as i32 + i - 13) as usize].norm_sqr()))
            .fold((0, 0.0), |a, b| if b.1 > a.1 { b } else { a });
        if !verbose {
            continue;
        }
        println!(
            "{k:2} tone {tone} true {:6.1} dB  other {:6.1} dB  peak {:+6.1} Hz {:6.1} dB{}",
            10.0 * p[tone as usize].log10(),
            10.0 * other.log10(),
            bi as f32 * 1.5625,
            10.0 * f32::log10(bp),
            if other > p[tone as usize] {
                "  <-- wrong"
            } else {
                ""
            }
        );
    }

    // What the decoder's soft bits make of it.
    let cw = ldpc::encode(crc::with_crc(m));
    let mut dec = ft8::Decoder::new(ft8::Config::default());
    if a.get(5).is_some_and(|v| v == "grid") {
        // Wrong bits (best of nsym 1-3) and BP success around f0/dt.
        print!("{:>8}", "dt\\df");
        let dfs: Vec<f32> = (-8..=8).map(|i| i as f32 * 0.125).collect();
        for df in &dfs {
            print!("{df:>7.2}");
        }
        println!();
        for i in -6..=6 {
            let ddt = i as f32 * 0.0025;
            print!("{:>8.4}", dt + ddt);
            for df in &dfs {
                let an = dec.analyze_at(&x, f0 + df, dt + ddt);
                let mut best = usize::MAX;
                let mut ok = false;
                for llr in &an.metrics {
                    best = best.min(
                        (0..ldpc::N)
                            .filter(|&i| (llr[i] > 0.0) != ldpc::get(&cw_of(m), i))
                            .count(),
                    );
                    ok |= ldpc::Bp::default().decode(llr, 30).errors == 0;
                }
                print!("{:>6}{}", best, if ok { "*" } else { " " });
            }
            println!();
        }
        return;
    }
    let an = dec.analyze(&x, f0, dt);
    println!(
        "decoder: f0 {:.2} dt {:.3} nsync {}",
        an.freq, an.dt, an.nsync
    );
    for (k, llr) in an.metrics.iter().enumerate() {
        let wrong: Vec<f32> = (0..ldpc::N)
            .filter(|&i| (llr[i] > 0.0) != ldpc::get(&cw, i))
            .map(|i| llr[i].abs())
            .collect();
        let bp = ldpc::Bp::default().decode(llr, 30);
        let scales: Vec<String> = [0.5f32, 0.75, 1.5, 2.0, 3.0]
            .iter()
            .map(|&g| {
                let l: [f32; ldpc::N] = std::array::from_fn(|i| llr[i] * g);
                format!("x{g}:{}", ldpc::Bp::default().decode(&l, 50).errors)
            })
            .collect();
        println!("  bp by scale {}", scales.join(" "));
        let mut res = vec![];
        for (o1, o2) in [(91, 0), (91, 20), (91, 40), (91, 91)] {
            let f = osd::decode(
                llr,
                osd::Depth {
                    order1: o1,
                    order2: o2,
                },
            );
            res.push(match f {
                Some(f) if f.codeword == cw => {
                    format!("({o1},{o2}) ok d={:.0} h={}", f.distance, f.hard_errors)
                }
                Some(f) => format!("({o1},{o2}) WRONG d={:.0}", f.distance),
                None => format!("({o1},{o2}) -"),
            });
        }
        println!(
            "nsym {}: {} wrong bits, |llr| sum {:.0} max {:.1}; bp errors {}; osd {}",
            k + 1,
            wrong.len(),
            wrong.iter().sum::<f32>(),
            wrong.iter().cloned().fold(0.0, f32::max),
            bp.errors,
            res.join(", ")
        );
    }
}

fn cw_of(m: u128) -> ldpc::Bits {
    ldpc::encode(crc::with_crc(m))
}
