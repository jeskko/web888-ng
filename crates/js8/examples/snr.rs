// SPDX-License-Identifier: GPL-3.0-or-later
//! Decode rate against SNR (in 2500 Hz) per submode, on synthetic frames
//! in white noise: `snr [TRIALS] [SUBMODES]`. Also counts frames decoded
//! from noise alone (false decodes) in as many periods.
use web888_ftx::js8;
use web888_ftx::tones::{self, Shape};
use web888_js8::{Frames, Submode};

struct Rng(u64);
impl Rng {
    fn bits(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn uniform(&mut self) -> f64 {
        (self.bits() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn gauss(&mut self) -> f32 {
        let (u, v) = (self.uniform().max(1e-300), self.uniform());
        ((-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()) as f32
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let trials: usize = a.get(1).map_or(20, |s| s.parse().unwrap());
    let modes: Vec<Submode> = match a.get(2) {
        Some(s) => s
            .split(',')
            .map(|n| *Submode::ALL.iter().find(|m| m.name() == n).unwrap())
            .collect(),
        None => Submode::ALL.to_vec(),
    };
    let mut rng = Rng(12345);
    let mut frames = Frames::new();
    for sm in modes {
        let n = sm.period() as usize * 12000;
        print!("{:6}", sm.name());
        for snr in [-14.0f32, -18.0, -20.0, -22.0, -24.0, -26.0, -28.0] {
            let mut ok = 0;
            for _ in 0..trials {
                let m: u128 = ((rng.bits() as u128) << 11 ^ rng.bits() as u128) & ((1 << 75) - 1);
                let t = js8::tones(js8::with_crc(m), &sm.costas());
                let f0 = 300.0 + 2400.0 * rng.uniform() as f32;
                let dt = (rng.uniform() as f32 - 0.5) * 0.4;
                let start = ((sm.start() + dt) * 12000.0) as usize;
                let amp = (2.0 * 10f32.powf(snr / 10.0) * 2500.0 / 6000.0).sqrt();
                let mut x = tones::synth(
                    &t,
                    sm.baud() as f64,
                    Shape::Fsk,
                    f0 as f64,
                    12000.0,
                    start,
                    n,
                    amp,
                );
                for v in x.iter_mut() {
                    *v += rng.gauss();
                }
                ok += frames
                    .decode(sm, &x)
                    .iter()
                    .any(|f| f.bits == m && (f.freq - f0).abs() < 2.0)
                    as usize;
            }
            print!(
                "  {snr:+3.0} dB {:3.0} %",
                100.0 * ok as f32 / trials as f32
            );
        }
        let mut false_ = 0;
        for _ in 0..trials {
            let x: Vec<f32> = (0..n).map(|_| rng.gauss()).collect();
            false_ += frames.decode(sm, &x).len();
        }
        println!("  | noise: {false_} false in {trials} periods");
    }
}
