// SPDX-License-Identifier: GPL-3.0-or-later
//! The iq backend in bins mode (`doc/m6-channelizer.md` step 2), through
//! the `BinRadio` trait on a direct source.

use num_complex::Complex32;
use web888_iq::source::IqSource;
use web888_iq::{IqConfig, IqRadio};
use web888_radio::RX_SINE_FULL_SCALE;
use web888_radio::bins::BinRadio;

/// A tone at 7.074 MHz, 0.1 of full scale, for input around 8.6 MHz.
struct Tone(u64);
impl IqSource for Tone {
    fn read(&mut self, buf: &mut [Complex32]) -> std::io::Result<usize> {
        for o in buf.iter_mut() {
            let ph = (std::f64::consts::TAU * (7.074e6 - 8.6e6) / 7.68e6 * self.0 as f64)
                .rem_euclid(std::f64::consts::TAU);
            *o = Complex32::from_polar(0.1, ph as f32);
            self.0 += 1;
        }
        Ok(buf.len())
    }
}

#[test]
fn bins_mode_channelizes_the_input() {
    let mut cfg = IqConfig::new(8.6e6, 7.68e6);
    cfg.dc_block = false;
    cfg.bins = true;
    let mut r = IqRadio::direct(cfg, Box::new(Tone(0))).unwrap();
    let (k, _) = r.bin_caps().bin_of(7.074e6).unwrap();
    let g = r.set_bins(&[k]).unwrap();
    r.set_power(60).unwrap();
    let mut f = vec![];
    while f.len() < 200 {
        r.read_bins(&mut f).unwrap();
    }
    let s = f.last().unwrap();
    assert_eq!((s.generation, s.bins[0]), (g, k));
    let a = (s.iq[0].i as f64).hypot(s.iq[0].q as f64) / RX_SINE_FULL_SCALE;
    assert!((a - 0.1).abs() < 1e-3, "{a}");
    let mut line = vec![];
    assert!(r.read_power(&mut line).unwrap().is_some());
    assert!((10.0 * line[k].log10() + 20.0).abs() < 0.1, "{}", line[k]);
}

#[test]
fn without_bins_mode_the_calls_fail() {
    let mut r = IqRadio::direct(IqConfig::new(8.6e6, 7.68e6), Box::new(Tone(0))).unwrap();
    assert!(r.set_bins(&[1]).is_err());
    assert!(r.read_bins(&mut vec![]).is_err());
    assert_eq!(r.bin_caps().bins, 0);
}

/// Tones (Hz, amplitude) and complex Gaussian noise (σ per part) around
/// 8.6 MHz at 7.68 MS/s.
struct Scene {
    k: u64,
    tones: Vec<(f64, f32)>,
    sigma: f32,
    rng: u64,
}

impl Scene {
    fn uniform(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        ((self.rng >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
}

impl IqSource for Scene {
    fn read(&mut self, buf: &mut [Complex32]) -> std::io::Result<usize> {
        for o in buf.iter_mut() {
            let mut z = Complex32::default();
            for &(f, a) in &self.tones {
                let ph = (std::f64::consts::TAU * (f - 8.6e6) / 7.68e6 * self.k as f64)
                    .rem_euclid(std::f64::consts::TAU);
                z += Complex32::from_polar(a, ph as f32);
            }
            let r = (-2.0 * self.uniform().ln()).sqrt() as f32 * self.sigma;
            let t = (std::f64::consts::TAU * self.uniform()) as f32;
            *o = z + Complex32::from_polar(r, t);
            self.k += 1;
        }
        Ok(buf.len())
    }
}

/// A zoom job's levels, summed and normalised as a view does, against
/// `WfEngine`'s for the same frames of the same bins: within `tol` dB at
/// every point.
fn zoom_equals_wf(scene: Scene, tol: f32) {
    use web888_dsp::wf::{WfEngine, WfSpec};
    use web888_dsp::zoom::ZoomAcc;
    use web888_radio::bins::ZoomJob;
    let mut cfg = IqConfig::new(8.6e6, 7.68e6);
    cfg.dc_block = false;
    cfg.bins = true;
    let mut r = IqRadio::direct(cfg, Box::new(scene)).unwrap();
    assert!(r.zoom_caps().is_some());
    // bins 204..=206: 7.05 to 7.11 MHz
    let k0 = 204;
    let (logn, nb) = (7u8, 3usize);
    let n = 1usize << logn;
    r.set_bins(&[k0, k0 + 1, k0 + 2]).unwrap();
    let mut f = vec![];
    while !f
        .last()
        .is_some_and(|x: &web888_radio::bins::BinFrames| x.bins.len() == 3)
    {
        r.read_bins(&mut f).unwrap();
    }
    let job = ZoomJob {
        tag: 9,
        shift: 3,
        logn,
        bins: nb as u16,
        k0: k0 as u16,
    };
    assert_eq!(r.submit_zoom(&[job]).unwrap(), 1);
    let mut res = vec![];
    while res.is_empty() {
        r.read_bins(&mut f).unwrap();
        r.read_zoom(&mut res).unwrap();
    }
    let z = &res[0];
    assert_eq!((z.tag, z.saturated, z.power.len()), (9, false, nb * n / 2));
    let mut acc = ZoomAcc::new(nb, n);
    acc.add(0, &z.power);
    // the job's frames, from the bins stream
    let frames = web888_radio::bins::split(&f);
    let at = frames
        .iter()
        .position(|x| x.frame as u16 == z.frame && x.bins.len() == 3)
        .expect("the job's first frame");
    let spec = WfSpec {
        fs: 60e3,
        bins: n,
        budget: 1,
        fps: 20.0,
        droop: web888_dsp::chan::Droop::None,
        fft_per_bin: 1,
    };
    for b in 0..nb {
        let mut e = WfEngine::with_fft_size(spec, n);
        e.push_rx(frames[at..at + n].iter().map(|x| x.iq[b]), true);
        let l = e.line().unwrap();
        assert_eq!(l.ffts, 1);
        let want = &l.db[n / 4..3 * n / 4];
        let mut got = vec![];
        assert!(acc.bin_db(b, &mut got));
        for (j, (g, w)) in got.iter().zip(want).enumerate() {
            assert!((g - w).abs() < tol, "bin {b} point {j}: {g} vs {w} dB");
        }
    }
}

#[test]
fn zoom_levels_equal_the_waterfalls_on_a_tone() {
    // -20 dBFS at 7.074 MHz (bin 205, 4 kHz up), a low floor
    zoom_equals_wf(
        Scene {
            k: 0,
            tones: vec![(7.074e6, 0.1)],
            sigma: 1e-4,
            rng: 7,
        },
        0.1,
    );
}

#[test]
fn zoom_levels_equal_the_waterfalls_on_noise() {
    zoom_equals_wf(
        Scene {
            k: 0,
            tones: vec![],
            sigma: 3e-3,
            rng: 11,
        },
        0.1,
    );
}
