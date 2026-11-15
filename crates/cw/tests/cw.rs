// SPDX-License-Identifier: GPL-3.0-or-later
//! datarx's CW tests (`tests/test_cw.py`): speeds, prosigns, hand keying
//! and noise, noise only, a two-operator QSO; plus the skimmer on audio.

use num_complex::Complex32;
use web888_cw::dsp::Rng;
use web888_cw::synth::{self, Channel, Keying};
use web888_cw::{AUDIO_RATE, DecodeOpts, ENV_FS, Skimmer, decode};

const TEXT: &str = "CQ CQ DE N0CALL N0CALL PSE K";

/// Character error rate (spaces ignored).
fn cer(r: &str, h: &str) -> f64 {
    let a: Vec<char> = r.chars().filter(|&c| c != ' ').collect();
    let b: Vec<char> = h.chars().filter(|&c| c != ' ').collect();
    let mut d: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let prev = d.clone();
        d[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            d[j + 1] = (prev[j + 1] + 1)
                .min(d[j] + 1)
                .min(prev[j] + usize::from(ca != cb));
        }
    }
    d[b.len()] as f64 / a.len().max(1) as f64
}

fn cplx(text: &str, k: &Keying, ch: &Channel, seed: u64) -> Vec<Complex32> {
    synth::complex(text, k, ch, ENV_FS, &mut Rng::new(seed))
}

#[test]
fn clean_speeds() {
    for wpm in [12.0, 20.0, 30.0, 40.0] {
        let x = cplx(TEXT, &Keying::new(wpm), &Channel::new(50.0), wpm as u64);
        let r = decode(&x, &DecodeOpts::default());
        assert_eq!(r.text, TEXT, "{wpm} WPM");
        assert!((r.wpm - wpm).abs() < 0.08 * wpm, "{wpm}: {}", r.wpm);
        assert!((r.freq - 50.0).abs() < 1.0, "{}", r.freq);
    }
}

#[test]
fn prosigns_and_punctuation() {
    let t = "R R UR RST 579 = TNX FER QSO <SK>";
    let x = cplx(t, &Keying::new(25.0), &Channel::new(0.0), 3);
    assert_eq!(decode(&x, &DecodeOpts::default()).text, t);
}

#[test]
fn hand_keying_and_noise() {
    let mut k = Keying::new(18.0);
    k.jitter = 0.1;
    let ch = Channel {
        snr_db: Some(3.0),
        ..Channel::new(-120.0)
    };
    let r = decode(&cplx(TEXT, &k, &ch, 5), &DecodeOpts::default());
    assert!(cer(TEXT, &r.text) < 0.05, "{}", r.text);
    let ch = Channel {
        snr_db: Some(-3.0),
        ..Channel::new(60.0)
    };
    let r = decode(
        &cplx(TEXT, &Keying::new(25.0), &ch, 6),
        &DecodeOpts::default(),
    );
    assert!(cer(TEXT, &r.text) < 0.1, "{}", r.text);
    assert!((r.snr + 3.0).abs() < 3.0, "snr {}", r.snr);
}

#[test]
fn noise_only() {
    let mut rng = Rng::new(9);
    let x: Vec<Complex32> = (0..(10.0 * ENV_FS) as usize)
        .map(|_| Complex32::new(rng.normal() as f32, rng.normal() as f32))
        .collect();
    let r = decode(
        &x,
        &DecodeOpts {
            f0: Some(0.0),
            strict: true,
            ..Default::default()
        },
    );
    assert_eq!(r.text, "");
}

#[test]
fn qso_two_operators() {
    let a = cplx(
        "CQ DE N0CALL N0CALL K",
        &Keying::new(26.0),
        &Channel::new(0.0),
        1,
    );
    let b = cplx(
        "N0CALL DE N0CALL/P N0CALL/P K",
        &Keying::new(17.0),
        &Channel::new(20.0),
        2,
    );
    let mut x = a;
    x.extend(std::iter::repeat_n(
        Complex32::new(0.0, 0.0),
        (2.5 * ENV_FS) as usize,
    ));
    x.extend(b);
    let r = decode(
        &x,
        &DecodeOpts {
            f0: Some(0.0),
            span: 40.0,
            ..Default::default()
        },
    );
    assert_eq!(
        r.text,
        "CQ DE N0CALL N0CALL K N0CALL DE N0CALL/P N0CALL/P K"
    );
    assert!((r.wpm - 17.0).abs() < 2.0, "{}", r.wpm);
}

/// Error rates at the README's operating points (five messages each):
/// -3 dB under 10 %, 10 % jitter at 3 dB under 3 %, fading at 8 dB under
/// 12 %.
#[test]
fn error_rates() {
    let msgs = [
        "CQ CQ DE N0CALL N0CALL PSE K",
        "UR RST 579 579 NAME JOHN QTH OULU",
        "TNX FER QSO 73 ES GL DE N0CALL/P",
        "RIG IC7300 PWR 100W ANT DIPOLE",
        "WX CLOUDY TEMP 5C = HW CPY?",
    ];
    let run = |k: &Keying, ch: &Channel| -> f64 {
        msgs.iter()
            .enumerate()
            .map(|(i, m)| {
                let r = decode(&cplx(m, k, ch, 100 + i as u64), &DecodeOpts::default());
                cer(m, &r.text)
            })
            .sum::<f64>()
            / msgs.len() as f64
    };
    let e = run(
        &Keying::new(22.0),
        &Channel {
            snr_db: Some(-3.0),
            ..Channel::new(30.0)
        },
    );
    assert!(e < 0.10, "-3 dB: {e}");
    let k = Keying {
        jitter: 0.1,
        ..Keying::new(20.0)
    };
    let e = run(
        &k,
        &Channel {
            snr_db: Some(3.0),
            ..Channel::new(-40.0)
        },
    );
    assert!(e < 0.03, "jitter: {e}");
    let e = run(
        &Keying::new(20.0),
        &Channel {
            snr_db: Some(8.0),
            fade_hz: 0.2,
            ..Channel::new(0.0)
        },
    );
    assert!(e < 0.12, "fading: {e}");
}

/// Two stations in an SSB-wide audio passband, repeating: both found,
/// both decode, their frequencies and speeds right.
#[test]
fn skimmer_two_signals_in_audio() {
    let mut rng = Rng::new(7);
    let n = (40.0 * AUDIO_RATE) as usize;
    let mut audio = synth::audio_noise(n, AUDIO_RATE, 0.1, 10.0, &mut rng);
    for (hz, wpm, text) in [
        (700.0, 22.0, "CQ DE N0CALL N0CALL K"),
        (1300.0, 16.0, "TEST DE N0CALL/P N0CALL/P"),
    ] {
        let mut k = Keying::new(wpm);
        k.lead = 0.5;
        k.tail = 1.0;
        let s = synth::audio(text, &k, &Channel::new(hz), AUDIO_RATE, 0.1, &mut rng);
        for (i, a) in audio.iter_mut().enumerate() {
            *a += s[i % s.len()];
        }
    }
    let mut sk = Skimmer::new(200.0, 2800.0);
    let mut text = std::collections::HashMap::<u32, (f64, f64, String)>::new();
    for chunk in audio.chunks(1024) {
        sk.push(chunk);
        for e in sk.poll() {
            let t = text.entry(e.id).or_default();
            t.0 = e.hz;
            t.1 = e.wpm;
            t.2.push_str(&e.text);
        }
    }
    let mut got: Vec<_> = text.into_values().collect();
    got.sort_by(|a, b| a.0.total_cmp(&b.0));
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(
        (got[0].0 - 700.0).abs() < 3.0 && (got[1].0 - 1300.0).abs() < 3.0,
        "{got:?}"
    );
    assert!(
        (got[0].1 - 22.0).abs() < 2.0 && (got[1].1 - 16.0).abs() < 2.0,
        "{got:?}"
    );
    assert!(got[0].2.contains("N0CALL N0CALL"), "{got:?}");
    assert!(got[1].2.contains("N0CALL/P N0CALL/P"), "{got:?}");
    assert_eq!(sk.active().len(), 2);
}

/// Noise and a steady carrier: nothing.
#[test]
fn skimmer_quiet_on_noise_and_carriers() {
    let mut rng = Rng::new(8);
    let n = (30.0 * AUDIO_RATE) as usize;
    let mut audio = synth::audio_noise(n, AUDIO_RATE, 0.1, 10.0, &mut rng);
    for (i, a) in audio.iter_mut().enumerate() {
        *a += 0.1 * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / AUDIO_RATE as f32).cos();
    }
    let mut sk = Skimmer::default();
    let mut ev = Vec::new();
    for chunk in audio.chunks(960) {
        sk.push(chunk);
        ev.extend(sk.poll());
    }
    assert!(ev.is_empty(), "{ev:?}");
}

/// The neural decoder in the skimmer: a weak, hand-keyed station the
/// classical decoder can't read is decoded; noise stays quiet.
#[test]
fn skimmer_neural_weak_signal() {
    let text = "CQ CQ DE N0CALL N0CALL K";
    let run = |neural: bool, with_signal: bool| -> String {
        let mut rng = Rng::new(11);
        let n = (40.0 * AUDIO_RATE) as usize;
        let mut audio = synth::audio_noise(n, AUDIO_RATE, 0.1, -7.0, &mut rng);
        if with_signal {
            let k = Keying {
                jitter: 0.1,
                lead: 0.5,
                tail: 1.5,
                ..Keying::new(20.0)
            };
            let s = synth::audio(text, &k, &Channel::new(800.0), AUDIO_RATE, 0.1, &mut rng);
            for (i, a) in audio.iter_mut().enumerate() {
                *a += s[i % s.len()];
            }
        }
        let mut sk = Skimmer::new(200.0, 2800.0);
        sk.min_db = 8.0; // weak: detection from the spectrogram
        sk.neural = neural;
        let mut out = String::new();
        for c in audio.chunks(1024) {
            sk.push(c);
            for e in sk.poll() {
                out.push_str(&e.text);
            }
        }
        out
    };
    let neural = run(true, true);
    assert!(neural.contains("N0CALL N0CALL"), "neural: {neural:?}");
    let quiet = run(true, false);
    assert!(quiet.trim().is_empty(), "noise: {quiet:?}");
}

/// CW at `hz` in complex IQ at `rate` (edges smoothed over ~2 ms).
fn iq_signal(text: &str, wpm: f64, hz: f64, rate: f64, amp: f32, rng: &mut Rng) -> Vec<Complex32> {
    let k = Keying {
        lead: 0.5,
        tail: 1.0,
        ..Keying::new(wpm)
    };
    let key = synth::keying(text, &k, rate, rng);
    // two moving averages: a triangle ~2 ms each side
    let m = (0.002 * rate) as usize;
    let avg = |x: &[f32]| -> Vec<f32> {
        let mut c = vec![0f64; x.len() + 1];
        for i in 0..x.len() {
            c[i + 1] = c[i] + x[i] as f64;
        }
        (0..x.len())
            .map(|i| ((c[(i + m).min(x.len())] - c[i]) / m as f64) as f32)
            .collect()
    };
    let key = avg(&avg(&key));
    let ph0 = rng.uniform() * std::f64::consts::TAU;
    key.iter()
        .enumerate()
        .map(|(i, &a)| {
            let p = 2.0 * std::f64::consts::PI * hz * i as f64 / rate + ph0;
            Complex32::new((a * amp) * p.cos() as f32, (a * amp) * p.sin() as f32)
        })
        .collect()
}

/// Six stations over 40 kHz of a 60 kS/s channelizer bin, 14-34 WPM.
#[test]
fn skimmer_wide_iq() {
    let rate = 60_000.0;
    let mut rng = Rng::new(21);
    let n = (40.0 * rate) as usize;
    let sd = 0.003f32;
    let mut x: Vec<Complex32> = (0..n)
        .map(|_| Complex32::new(sd * rng.normal() as f32, sd * rng.normal() as f32))
        .collect();
    let st = [
        (-18_000.0, 14.0, "CQ DE N0CALL N0CALL K"),
        (-9_150.0, 22.0, "TEST N1CALL N1CALL"),
        (-2_000.0, 30.0, "CQ CQ N2CALL N2CALL"),
        (3_210.0, 18.0, "N3CALL DE N4CALL K"),
        (11_000.0, 26.0, "TU N5CALL TEST"),
        (18_800.0, 34.0, "CQ N6CALL N6CALL"),
    ];
    for &(hz, wpm, text) in &st {
        let s = iq_signal(text, wpm, hz, rate, 0.05, &mut rng);
        for (i, v) in x.iter_mut().enumerate() {
            *v += s[i % s.len()];
        }
    }
    let mut sk = Skimmer::new_iq(rate, -20_000.0, 20_000.0);
    let mut got = std::collections::HashMap::<u32, (f64, String)>::new();
    let t = std::time::Instant::now();
    for c in x.chunks(2048) {
        sk.push_iq(c);
        for e in sk.poll() {
            let g = got.entry(e.id).or_default();
            g.0 = e.hz;
            g.1.push_str(&e.text);
        }
    }
    eprintln!("40 s of a 60 kS/s bin: {:.1} s", t.elapsed().as_secs_f64());
    for &(hz, _, text) in &st {
        let call = text.split(' ').find(|w| w.contains("CALL")).unwrap();
        assert!(
            got.values()
                .any(|(f, t)| (f - hz).abs() < 10.0 && t.contains(call)),
            "{hz}: {got:?}"
        );
    }
}
