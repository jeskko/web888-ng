// SPDX-License-Identifier: GPL-3.0-or-later
//! The identifier's cost: one 4.096 s window, natively.
//!
//!     cargo run --release -p web888-sigid --example speed

use num_complex::Complex32;
use std::time::Instant;
use web888_sigid::{N, SigId, identify};

fn main() {
    let x: Vec<Complex32> = (0..N)
        .map(|i| {
            let a = 0.3 * i as f32;
            Complex32::new(a.cos(), a.sin())
        })
        .collect();
    let _ = SigId::get();
    let t = Instant::now();
    let n = 10;
    let mut top = "";
    for _ in 0..n {
        top = identify(&x)[0].0;
    }
    println!(
        "{:.0} ms a window ({top})",
        1000.0 * t.elapsed().as_secs_f64() / n as f64
    );
}
