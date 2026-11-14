// SPDX-License-Identifier: GPL-3.0-or-later
//! Image modes end to end: synthetic SSTV (every mode) and HF fax (APT
//! start, phasing, image, stop) through the audio path, with a sender
//! clock a few hundred ppm off and noise.

use web888_cw::dsp::Rng;
use web888_img::Imager;
use web888_img::fax::{FaxEvent, lum_of};
use web888_img::fm::{AUDIO_RATE, tone_audio};
use web888_img::sstv::{self, MODES, SstvEvent};

fn pattern(w: usize, h: usize) -> impl Fn(usize, usize) -> [u8; 3] {
    move |x, y| {
        let r = (x * 255 / w) as u8;
        let g = (y * 255 / h) as u8;
        let b = (128.0 + 100.0 * ((x + y) as f64 / 40.0).sin()) as u8;
        [r, g, b]
    }
}

fn noisy(audio: &mut [f32], sd: f64, seed: u64) {
    let mut rng = Rng::new(seed);
    for v in audio {
        *v += (sd * rng.normal()) as f32;
    }
}

/// Decode audio; the last SSTV image (mode name, RGB rows).
fn run_sstv(audio: &[f32]) -> Option<(&'static str, usize, Vec<u8>)> {
    let mut im = Imager::new();
    let mut cur: Option<(&'static str, usize, Vec<u8>)> = None;
    for c in audio.chunks(2048) {
        im.push(c);
        for e in im.poll().1 {
            match e {
                SstvEvent::Image { mode, width, .. } => cur = Some((mode, width, Vec::new())),
                SstvEvent::Rows { first, data } => {
                    let c = cur.as_mut().expect("image");
                    assert_eq!(c.2.len(), first * c.1 * 3, "rows in order");
                    c.2.extend(data);
                }
                SstvEvent::Redraw => cur.as_mut().expect("image").2.clear(),
                SstvEvent::Done { .. } => {}
            }
        }
    }
    cur
}

fn mean_err(rows: &[u8], w: usize, h: usize, px: &dyn Fn(usize, usize) -> [u8; 3]) -> f64 {
    let mut e = 0.0;
    let mut n = 0;
    // the edges blur (FM bandwidth): leave a margin
    for y in 0..h.min(rows.len() / (3 * w)) {
        for x in 8..w - 8 {
            let want = px(x, y);
            for c in 0..3 {
                e += (rows[(y * w + x) * 3 + c] as f64 - want[c] as f64).abs();
                n += 1;
            }
        }
    }
    e / n as f64
}

#[test]
fn sstv_every_mode() {
    for m in MODES {
        let px = pattern(m.width, m.height);
        let f = sstv::encode(m, &px, AUDIO_RATE);
        let mut audio = vec![0.0; 6000];
        audio.extend(tone_audio(&f, 0.3));
        noisy(&mut audio, 0.01, 1);
        let (name, w, rows) = run_sstv(&audio).unwrap_or_else(|| panic!("{}: no image", m.name));
        assert_eq!(name, m.name);
        assert_eq!(rows.len(), w * m.height * 3, "{}: all rows", m.name);
        let e = mean_err(&rows, w, m.height, &px);
        assert!(e < 12.0, "{}: mean error {e:.1}", m.name);
    }
}

/// A sender clock 400 ppm off and noise: the sync fit takes the slant out.
#[test]
fn sstv_slant_and_noise() {
    for name in ["Martin 1", "Scottie 1", "Robot 36", "PD 120"] {
        let m = MODES.iter().find(|m| m.name == name).expect("mode");
        let px = pattern(m.width, m.height);
        let f = sstv::encode(m, &px, AUDIO_RATE * (1.0 + 400e-6));
        let mut audio = vec![0.0; 3000];
        audio.extend(tone_audio(&f, 0.3));
        noisy(&mut audio, 0.05, 2); // ~15 dB in 3 kHz
        let (got, w, rows) = run_sstv(&audio).unwrap_or_else(|| panic!("{name}: no image"));
        assert_eq!(got, name);
        let e = mean_err(&rows, w, m.height, &px);
        assert!(e < 30.0, "{name}: mean error {e:.1}");
    }
}

/// A fax as DWD sends it: start tone, 30 s of phasing (a black pulse on white,
/// or the reverse), the chart, stop tone; the sender's clock off.
fn fax_freq(rate: f64, lines: usize, dark_pulse: bool) -> Vec<f32> {
    let w = 1810;
    // every sample from its exact time (a line is 0.5 s of the sender's clock)
    let (t_tone, t_phase, t_img) = (1.0, 6.0, 36.0);
    let t_stop = t_img + 0.5 * lines as f64;
    let n = ((t_stop + 7.0) * rate) as usize;
    (0..n)
        .map(|i| {
            let t = i as f64 / rate;
            let sq = |hz: f64| {
                if (t * hz).fract() < 0.5 {
                    1500.0
                } else {
                    2300.0
                }
            };
            if t < t_tone {
                1900.0
            } else if t < t_phase {
                sq(300.0)
            } else if t < t_img {
                let pulse = ((t - t_phase) / 0.5).fract() < 0.05;
                if pulse != dark_pulse { 2300.0 } else { 1500.0 }
            } else if t < t_stop {
                let y = ((t - t_img) / 0.5) as usize;
                let x = (((t - t_img) / 0.5).fract() * w as f64) as usize;
                1500.0 + 800.0 * fax_px(x, y)
            } else if t < t_stop + 5.0 {
                sq(450.0)
            } else {
                1500.0
            }
        })
        .collect()
}

/// The chart: vertical and slanted bars (slant shows as their drift).
fn fax_px(x: usize, y: usize) -> f32 {
    if (x / 60).is_multiple_of(2) || ((x + y) / 90).is_multiple_of(3) {
        1.0
    } else {
        0.0
    }
}

#[test]
fn fax_apt_phasing_slant() {
    for (ppm, dark) in [(0.0, true), (300.0, true), (-250.0, false)] {
        let f = fax_freq(AUDIO_RATE * (1.0 + ppm * 1e-6), 200, dark);
        let mut audio = tone_audio(&f, 0.3);
        noisy(&mut audio, 0.03, 3);
        let mut im = Imager::new();
        let mut rows: Vec<u8> = Vec::new();
        let (mut started, mut done, mut width) = (false, false, 0);
        for c in audio.chunks(2048) {
            im.push(c);
            for e in im.poll().0 {
                match e {
                    FaxEvent::Image {
                        width: w,
                        ioc,
                        auto,
                    } => {
                        assert!(auto && ioc == 576);
                        started = true;
                        width = w;
                        rows.clear();
                    }
                    FaxEvent::Rows { first, data } => {
                        assert_eq!(rows.len(), first * width);
                        rows.extend(data);
                    }
                    FaxEvent::Redraw => rows.clear(),
                    FaxEvent::Done { .. } => done = true,
                }
            }
        }
        assert!(started && done, "{ppm}");
        let slant = im.fax.slant();
        assert!((slant - ppm).abs() < 60.0, "{ppm} ppm: fitted {slant:.0}");
        // the chart's rows: after the 60 phasing lines (the image starts
        // within a line of the start tone's end)
        let n = rows.len() / width;
        let mut err = 0.0;
        let mut cnt = 0;
        for y in 65..(n - 10).min(255) {
            for x in (20..width - 20).step_by(7) {
                let want = fax_px(x, y - 60);
                let got = rows[y * width + x] as f32 / 255.0;
                err += (want - got).abs() as f64;
                cnt += 1;
            }
        }
        let err = err / cnt as f64;
        assert!(err < 0.15, "{ppm} ppm: mean error {err:.3}");
    }
}

#[test]
fn fax_by_hand() {
    // no APT: started by hand, nominal timing
    let mut f = Vec::new();
    for y in 0..60 {
        for i in 0..6000 {
            f.push(1500.0 + 800.0 * fax_px(i * 1810 / 6000, y));
        }
    }
    let audio = tone_audio(&f, 0.3);
    let mut im = Imager::new();
    im.fax.start_now();
    let mut n = 0;
    for c in audio.chunks(4096) {
        im.push(c);
        for e in im.poll().0 {
            if let FaxEvent::Rows { data, .. } = e {
                n += data.len() / 1810;
            }
        }
    }
    assert!(n >= 58, "{n} rows");
    assert_eq!(lum_of(1500.0), 0.0);
    assert_eq!(lum_of(2300.0), 1.0);
}
