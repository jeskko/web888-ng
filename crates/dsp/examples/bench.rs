// SPDX-License-Identifier: GPL-3.0-or-later
//! Per-session DSP cost, as % of one core in real time, plus a few raw
//! kernels to compare cores (doc/m3-multi-user.md §4).
//!   cargo run --release -p web888-dsp --example bench [seconds]
use std::hint::black_box;
use std::time::Instant;
use web888_dsp::C32;
use web888_dsp::audio::{AudioChain, AudioSpec, Mode};
use web888_dsp::chan::Droop;
use web888_dsp::pfb::{PfbChannel, PfbSpec};
use web888_dsp::wf::{WfEngine, WfSpec};
use web888_radio::Iq32;
use web888_radio::chan::{ADC_HZ_HF, rx_rate};

struct Noise(u32);
impl Noise {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
        (self.0 >> 8) as f32 / (1 << 24) as f32 - 0.5
    }
}

/// `secs` of RX samples: noise and a tone, at a typical HF level.
fn rx_input(fs: f64, secs: f64) -> Vec<Iq32> {
    let mut n = Noise(1);
    let a = 2.0e5;
    (0..(fs * secs) as usize)
        .map(|k| {
            let ph = 2.0 * std::f64::consts::PI * 1000.0 / fs * k as f64;
            Iq32 {
                i: (a * (ph.cos() as f32 + 0.3 * n.next())) as i32,
                q: (a * (ph.sin() as f32 + 0.3 * n.next())) as i32,
            }
        })
        .collect()
}

fn report(name: &str, dt: f64, secs: f64) {
    println!("{name:<32} {:6.2} % of a core", 100.0 * dt / secs);
}

fn audio(name: &str, spec: AudioSpec, fs: f64, rx: &[Iq32], secs: f64) {
    let droop = Droop::Rx {
        adc_hz: ADC_HZ_HF,
        r2: 10,
    };
    let mut chain = AudioChain::new(spec, fs, droop);
    let mut out = Vec::new();
    // 10 ms blocks, as the hw thread delivers them
    let block = (fs / 100.0) as usize;
    let t = Instant::now();
    for b in rx.chunks(block) {
        out.clear();
        chain.process(b.iter().copied(), &mut out);
        black_box(&out);
    }
    report(name, t.elapsed().as_secs_f64(), secs);
}

/// One waterfall view: `bins` per line at `fps`, `budget` FFTs per line,
/// fed `fs` samples/s.
fn waterfall(bins: usize, fps: f64, budget: usize, fs: f64, secs: f64) {
    let mut e = WfEngine::new(WfSpec {
        fs,
        bins,
        budget,
        fps,
        droop: Droop::None,
        fft_per_bin: 2,
    });
    let mut n = Noise(2);
    let per_line = (fs / fps) as usize;
    let input: Vec<C32> = (0..per_line)
        .map(|_| C32::new(n.next(), n.next()))
        .collect();
    let lines = (secs * fps) as usize;
    let t = Instant::now();
    for _ in 0..lines {
        e.push_c32(input.iter().copied(), true);
        black_box(e.line());
    }
    report(
        &format!("wf {bins} bins {fps} fps x{budget} {:.2} MS/s", fs / 1e6),
        t.elapsed().as_secs_f64(),
        secs,
    );
}

fn kernels() {
    // complex f32 FFT
    for n in [1024, 2048, 4096] {
        let fft = rustfft::FftPlanner::<f32>::new().plan_fft_forward(n);
        let mut buf = vec![C32::new(0.1, 0.2); n];
        let mut scratch = vec![C32::default(); fft.get_inplace_scratch_len()];
        let reps = 2_000_000 / n;
        let t = Instant::now();
        for _ in 0..reps {
            fft.process_with_scratch(&mut buf, &mut scratch);
            black_box(&buf);
        }
        let us = t.elapsed().as_secs_f64() * 1e6 / reps as f64;
        println!("fft {n:<5}                        {us:8.1} us");
    }
    // real FIR, f32 multiply-adds
    let taps: Vec<f32> = (0..256).map(|k| (k as f32 * 0.01).sin()).collect();
    let x: Vec<f32> = (0..4096 + 256).map(|k| (k as f32 * 0.37).cos()).collect();
    let reps = 200;
    let t = Instant::now();
    for _ in 0..reps {
        for i in 0..4096 {
            let s: f32 = taps.iter().zip(&x[i..]).map(|(a, b)| a * b).sum();
            black_box(s);
        }
    }
    let m = (reps * 4096 * 256) as f64 / t.elapsed().as_secs_f64() / 1e6;
    println!("fir f32 (sum of products)        {m:8.0} M MAC/s");
    // scalar integer work
    let reps = 50_000_000u32;
    let t = Instant::now();
    let mut s = 1u32;
    for k in 0..reps {
        s = black_box(s.wrapping_mul(1664525).wrapping_add(k) ^ (s >> 7));
    }
    let m = reps as f64 / t.elapsed().as_secs_f64() / 1e6;
    println!("int lcg (dependent chain)        {m:8.0} M it/s");
}

fn main() {
    let secs: f64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10.0);
    let fs = rx_rate(ADC_HZ_HF, 10);
    let rx = rx_input(fs, secs);
    println!("audio chain, {fs:.0} S/s in, {secs} s:");
    audio("usb", AudioSpec::new(Mode::Usb), fs, &rx, secs);
    audio("cw", AudioSpec::new(Mode::Cw), fs, &rx, secs);
    audio("am", AudioSpec::new(Mode::Am), fs, &rx, secs);
    let mut s = AudioSpec::new(Mode::Usb);
    s.nb = Some(8.0);
    audio("usb + nb", s, fs, &rx, secs);
    s.nb = None;
    s.nr = 0.6;
    audio("usb + nr", s, fs, &rx, secs);
    s.nb = Some(8.0);
    s.squelch_db = Some(-100.0);
    audio("usb + nb + nr + squelch", s, fs, &rx, secs);
    // the channelizer's per-user fine stage: a 60 kS/s bin in
    let spec = PfbSpec::web888();
    let bin: Vec<C32> = rx
        .iter()
        .take((spec.out_rate() * secs) as usize)
        .map(|s| web888_dsp::rx_to_c32(*s))
        .collect();
    let mut bin60 = bin.clone();
    // 48 kS/s samples stand in for 60 kS/s ones: same cost per sample
    bin60.extend(bin.iter().take(bin.len() / 4));
    let mut ch = PfbChannel::new(&spec, 7_074_000.0 + 7e3, AudioSpec::new(Mode::Usb));
    let mut out = Vec::new();
    let t = Instant::now();
    for b in bin60.chunks((spec.out_rate() / 100.0) as usize) {
        out.clear();
        ch.process(b, &mut out);
        black_box(&out);
    }
    report(
        "pfb fine stage usb (60k in)",
        t.elapsed().as_secs_f64(),
        secs,
    );
    println!("waterfall view:");
    waterfall(1024, 10.0, 30, 1.92e6, secs);
    waterfall(1024, 25.0, 30, 1.92e6, secs);
    waterfall(2048, 25.0, 30, 1.92e6, secs);
    waterfall(1024, 25.0, 30, 30.72e6, secs);
    waterfall(1024, 25.0, 8, 1.92e6, secs);
    println!("kernels:");
    kernels();
}
