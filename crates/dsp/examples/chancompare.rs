// SPDX-License-Identifier: GPL-3.0-or-later
//! Noise floor of a USB channel through the channelizer model against
//! today's tuned chain (exact model of the FPGA RX DDC: NCO, CIC N=3
//! R=256, CIC N=5 R=10), on the same ADC samples: white noise at the
//! LTC2208's SNR plus strong carriers at aliasing offsets around each
//! dial. hw/docs/channelizer.md §3.
//!   cargo run --release -p web888-dsp --example chancompare [seconds]
//! Environment: LEVEL=dBFS (carrier level, default -26), CARRIERS=i:j,...
//! (only these dial:offset pairs), NOISE=x (noise scale, 0 = none).
use num_complex::{Complex32, Complex64};
use web888_dsp::C32;
use web888_dsp::audio::{AudioChain, AudioSpec, Mode};
use web888_dsp::chan::Droop;
use web888_dsp::pfb::{Pfb, PfbChannel, PfbSpec};
use web888_iq::ddc::Ddc;

const FS: f64 = 122.88e6;
const DIALS: [f64; 6] = [3.573e6, 7.074e6, 10.136e6, 14.074e6, 18.1e6, 21.074e6];
/// Strong carriers relative to each dial: the channelizer's aliasing
/// offsets (folding by 60 kS/s) and the tuned chain's (by 48 kS/s).
const OFFSETS: [f64; 6] = [45e3, -46.5e3, 49.5e3, -97.5e3, 61.5e3, 130e3];
const CARRIER_DBFS: f64 = -26.0;

/// Carrier level: `LEVEL` (dBFS) or the default.
fn carrier_dbfs() -> f64 {
    std::env::var("LEVEL")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(CARRIER_DBFS)
}
/// LTC2208: about 77.7 dB SNR (full-scale sine to noise up to fs/2).
const ADC_SNR_DB: f64 = 77.7;

struct Gauss(u64);
impl Gauss {
    fn u(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    fn next(&mut self) -> f64 {
        (-2.0 * self.u().ln()).sqrt() * (2.0 * std::f64::consts::PI * self.u()).cos()
    }
}

/// ADC samples: noise, and the carriers if `carriers`.
struct Adc {
    noise: Gauss,
    sigma: f64,
    osc: Vec<(Complex64, Complex64)>,
}

impl Adc {
    fn new(carriers: bool) -> Self {
        let sigma = (0.5 * 10f64.powf(-ADC_SNR_DB / 10.0)).sqrt()
            * std::env::var("NOISE")
                .ok()
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(1.0);
        let a = 10f64.powf(carrier_dbfs() / 20.0);
        let mut osc = Vec::new();
        // CARRIERS="i:j,..." (dial, offset indices) limits the carriers
        let only: Option<Vec<(usize, usize)>> = std::env::var("CARRIERS").ok().map(|v| {
            v.split(',')
                .filter_map(|p| {
                    let (a, b) = p.split_once(':')?;
                    Some((a.parse().ok()?, b.parse().ok()?))
                })
                .collect()
        });
        if carriers {
            for (i, d) in DIALS.iter().enumerate() {
                for (j, o) in OFFSETS.iter().enumerate() {
                    if only.as_ref().is_some_and(|v| !v.contains(&(i, j))) {
                        continue;
                    }
                    let ph = (i * 7 + j * 3) as f64;
                    osc.push((
                        Complex64::from_polar(a, ph),
                        Complex64::from_polar(1.0, 2.0 * std::f64::consts::PI * (d + o) / FS),
                    ));
                }
            }
        }
        Adc {
            noise: Gauss(0x9e37_79b9_7f4a_7c15),
            sigma,
            osc,
        }
    }
    fn block(&mut self, out: &mut Vec<f32>, n: usize) {
        out.clear();
        for _ in 0..n {
            let mut x = self.sigma * self.noise.next();
            for (p, r) in &mut self.osc {
                x += p.re;
                *p *= *r;
            }
            out.push(x as f32);
        }
        // keep the oscillators on the unit circle
        for (p, _) in &mut self.osc {
            *p *= 10f64.powf(carrier_dbfs() / 20.0) / p.norm();
        }
    }
}

fn usb() -> AudioSpec {
    let mut s = AudioSpec::new(Mode::Usb);
    s.agc = None;
    s.gain_db = 0.0;
    s
}

/// USB audio power (dB, full-scale sine = 0 dB) per dial: (tuned, channelizer).
fn floors(carriers: bool, secs: f64) -> Vec<(f64, f64)> {
    let spec = PfbSpec::web888();
    let mut adc = Adc::new(carriers);
    let mut pfb = Pfb::new(spec);
    let mut ddc: Vec<Ddc> = DIALS.iter().map(|&d| Ddc::rx(d, FS, 256, 10)).collect();
    let mut tuned: Vec<AudioChain> = DIALS
        .iter()
        .map(|_| AudioChain::new(usb(), 48_000.0, Droop::Rx { adc_hz: FS, r2: 10 }))
        .collect();
    let mut chans: Vec<PfbChannel> = DIALS
        .iter()
        .map(|&d| PfbChannel::new(&spec, d, usb()))
        .collect();
    let mut acc = vec![(0.0, 0usize, 0.0, 0usize); DIALS.len()];
    let block = 2048 * 60; // 1 ms
    let blocks = (secs * FS) as usize / block;
    // the carriers switch on at t = 0; their onset splatters through both
    // chains, and the audio filter holds ~60 ms: measure from 200 ms on
    let settle = 200;
    assert!(blocks > 2 * settle, "run at least 0.4 s");
    let (mut x, mut xc, mut y, mut bins) = (
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![Vec::new(); DIALS.len()],
    );
    let mut audio = Vec::new();
    for b in 0..blocks {
        adc.block(&mut x, block);
        xc.clear();
        xc.extend(x.iter().map(|&v| Complex32::new(v, 0.0)));
        // tuned chain: the DDC has unit DC gain, a real sine gives half
        for (i, d) in ddc.iter_mut().enumerate() {
            d.process(&xc, &mut y);
            audio.clear();
            tuned[i].process_c32(
                y.iter()
                    .map(|z| C32::new(2.0 * z.re as f32, 2.0 * z.im as f32)),
                &mut audio,
            );
            if b >= settle {
                acc[i].0 += audio.iter().map(|v| (*v as f64).powi(2)).sum::<f64>();
                acc[i].1 += audio.len();
            }
        }
        for v in &mut bins {
            v.clear();
        }
        let ks: Vec<usize> = chans.iter().map(|c| c.bin()).collect();
        pfb.push(&x, |f| {
            for (i, &k) in ks.iter().enumerate() {
                bins[i].push(f[k]);
            }
        });
        for (i, c) in chans.iter_mut().enumerate() {
            audio.clear();
            c.process(&bins[i], &mut audio);
            if b >= settle {
                acc[i].2 += audio.iter().map(|v| (*v as f64).powi(2)).sum::<f64>();
                acc[i].3 += audio.len();
            }
        }
    }
    // full-scale sine = 0 dB: its audio is a sine of amplitude 1, power 0.5
    let db = |p: f64, n: usize| 10.0 * (p / n.max(1) as f64 / 0.5).log10();
    acc.iter().map(|a| (db(a.0, a.1), db(a.2, a.3))).collect()
}

fn main() {
    let secs: f64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.6);
    let quiet = floors(false, secs);
    let loud = floors(true, secs);
    println!(
        "USB 300-2700 Hz noise floor, dB re full-scale sine ({} carriers at {} dBFS, ADC SNR {ADC_SNR_DB} dB, {secs} s):",
        DIALS.len() * OFFSETS.len(),
        carrier_dbfs()
    );
    println!("dial kHz     tuned: noise  +carriers   channelizer: noise  +carriers");
    for (i, d) in DIALS.iter().enumerate() {
        println!(
            "{:9.1}   {:11.2}  {:9.2}   {:18.2}  {:9.2}",
            d / 1e3,
            quiet[i].0,
            loud[i].0,
            quiet[i].1,
            loud[i].1
        );
    }
}
