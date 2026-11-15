// SPDX-License-Identifier: GPL-3.0-or-later
use num_complex::Complex32;
use web888_hfdl::demod::Demod;
use web888_hfdl::front::Front;
use web888_hfdl::pdu::{self, Pdu};
use web888_hfdl::phy::Mode;
use web888_hfdl::synth::{self, Channel, Rng};

/// Bursts through audio at 12 kS/s; the squitters decoded.
fn run(mode: usize, ch: Channel, snr: f64, seed: u64) -> (usize, Vec<String>) {
    let mut rng = Rng(seed);
    let m = Mode::new(mode);
    let oct = synth::squitter(7, 1234);
    let sym = synth::symbols(m, &oct, &mut rng);
    let rate = 12000.0;
    let mut x = synth::iq(&sym, rate, 1440.0, ch, 0.5);
    let active = (sym.len() as f64 / 1800.0 * rate) as usize;
    synth::noise(&mut x, rate, snr, active, &mut rng);
    // audio: the real part
    let audio: Vec<Complex32> = x.iter().map(|v| Complex32::new(v.re, 0.0)).collect();
    let mut front = Front::new(rate, 1440.0);
    let mut demod = Demod::new();
    let mut y = Vec::new();
    let mut found = Vec::new();
    for c in audio.chunks(1000) {
        y.clear();
        front.push(c, &mut y);
        for b in demod.push(&y) {
            found.push(format!(
                "{:?} f {:.1} snr {:.1}",
                b.mode.index, b.freq, b.snr
            ));
            if let Some(Pdu::Squitter(s)) = pdu::parse(&b.octets) {
                assert_eq!((s.gs, s.frame_index), (7, 1234));
                found.push("ok".into());
            }
        }
    }
    (found.iter().filter(|s| *s == "ok").count(), found)
}

#[test]
fn clean_all_modes() {
    for mode in 0..8 {
        let (n, f) = run(mode, Channel::default(), 30.0, 1 + mode as u64);
        assert_eq!(n, 1, "mode {mode}: {f:?}");
    }
}

#[test]
fn offset_multipath_noise() {
    let ch = Channel {
        offset: 23.0,
        delay: 1.2e-3,
        gain: 0.6,
    };
    for mode in [1, 3] {
        let (n, f) = run(mode, ch, 12.0, 9);
        assert_eq!(n, 1, "mode {mode}: {f:?}");
    }
}
