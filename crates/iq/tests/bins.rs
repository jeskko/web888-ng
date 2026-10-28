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
