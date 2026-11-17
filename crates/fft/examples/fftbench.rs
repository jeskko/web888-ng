// SPDX-License-Identifier: GPL-3.0-or-later
//! Time `web888_fft::plan` against rustfft per size (run on the unit):
//! `fftbench [N...]`.
use num_complex::Complex32 as C32;
use std::time::Instant;

fn main() {
    let sizes: Vec<usize> = std::env::args()
        .skip(1)
        .map(|a| a.parse().expect("size"))
        .collect();
    let sizes = if sizes.is_empty() {
        vec![32, 256, 512, 1024, 2048, 4096]
    } else {
        sizes
    };
    let mut p = rustfft::FftPlanner::new();
    for n in sizes {
        let ours = web888_fft::plan(n, false);
        let theirs = p.plan_fft_forward(n);
        let reps = (4_000_000 / n).max(10);
        let mut x: Vec<C32> = (0..n)
            .map(|i| C32::new((i % 7) as f32, (i % 3) as f32))
            .collect();
        let time = |f: &dyn rustfft::Fft<f32>, x: &mut Vec<C32>| {
            let mut s = vec![C32::default(); f.get_inplace_scratch_len()];
            let t = Instant::now();
            for _ in 0..reps {
                f.process_with_scratch(x, &mut s);
            }
            t.elapsed().as_secs_f64() * 1e6 / reps as f64
        };
        let a = time(&*theirs, &mut x);
        let b = time(&*ours, &mut x);
        println!("{n:6}: rustfft {a:8.1} us, ours {b:8.1} us ({:.2}x)", a / b);
    }
}
