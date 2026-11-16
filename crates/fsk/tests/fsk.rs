// SPDX-License-Identifier: GPL-3.0-or-later
//! datarx's FSK tests (`tests/test_fsk.py`): parameter detection on
//! synthetic signals of many kinds, SITOR-B, reversals, noise, the
//! streaming decoder; plus the skimmer on audio.

use num_complex::Complex32;
use web888_cw::dsp::Rng;
use web888_fsk::analyze::{FskDecoder, FskParams, Mode, Parity, analyze, is_valid};
use web888_fsk::charset::{self, ita2_encode};
use web888_fsk::demod::{ToneDemod, estimate_baud, transitions};
use web888_fsk::skim::{AUDIO_RATE, Event, Skimmer};
use web888_fsk::synth::{self, Cond};

const LINE: &str = "CQ CQ TEST DE N0CALL N0CALL 599 001 KN\nTHE QUICK BROWN FOX JUMPS OVER THE LAZY DOG 0123456789 ";
const FS: f64 = 4000.0;
const FOX: &str = "THE QUICK BROWN FOX JUMPS OVER THE LAZY DOG 0123456789";

fn text() -> String {
    LINE.repeat(3)
}

fn ascii(t: &str) -> Vec<u32> {
    t.bytes().map(u32::from).collect()
}

fn run(seg: &synth::Seg, baud: f64, shift: f64, c: &Cond, seed: u64) -> Vec<Complex32> {
    synth::complex(seg, baud, shift, FS, c, &mut Rng::new(seed))
}

#[test]
fn framings() {
    let t = text();
    // name, codes, data bits, parity, stop, baud, shift, mark high, framing, charset
    type Case<'a> = (
        &'a str,
        Vec<u32>,
        u32,
        Parity,
        f64,
        f64,
        f64,
        bool,
        &'a str,
        &'a str,
    );
    let cases: Vec<Case> = vec![
        (
            "ham RTTY",
            ita2_encode(&t, true),
            5,
            Parity::None,
            1.5,
            45.45,
            170.0,
            true,
            "5N1.5",
            "ITA2",
        ),
        (
            "DWD",
            ita2_encode(&t, true),
            5,
            Parity::None,
            1.5,
            50.0,
            450.0,
            false,
            "5N1.5",
            "ITA2",
        ),
        (
            "75 Bd 5N1",
            ita2_encode(&t, true),
            5,
            Parity::None,
            1.0,
            75.0,
            850.0,
            true,
            "5N1",
            "ITA2",
        ),
        (
            "110 Bd 7E2",
            ascii(&t),
            7,
            Parity::Even,
            2.0,
            110.0,
            200.0,
            true,
            "7E2",
            "ASCII",
        ),
        (
            "300 Bd 8N1",
            ascii(&t),
            8,
            Parity::None,
            1.0,
            300.0,
            850.0,
            true,
            "8N1",
            "ASCII",
        ),
    ];
    for (name, codes, nd, par, stop, baud, shift, mh, framing, cs) in cases {
        let seg = synth::async_simple(&codes, nd, par, stop);
        let x = run(
            &seg,
            baud,
            shift,
            &Cond {
                snr_db: Some(10.0),
                mark_high: mh,
                ..Cond::new(37.0)
            },
            1,
        );
        let r = analyze(&x, FS, None, 0.0);
        let p = r
            .params
            .clone()
            .unwrap_or_else(|| panic!("{name}: {:?}", r.notes));
        assert!(is_valid(&r, 12), "{name}: {p:?} {:?}", r.notes);
        assert!((p.baud - baud).abs() < 0.01 * baud, "{name}: {}", p.baud);
        assert!((p.shift() - shift).abs() < 3.0, "{name}: {}", p.shift());
        assert!((0.5 * (p.f_lo + p.f_hi) - 37.0).abs() < 2.0, "{name}");
        assert_eq!(p.mark_high, mh, "{name}");
        assert_eq!(
            (p.framing().as_str(), p.charset.as_str()),
            (framing, cs),
            "{name}"
        );
        assert!(r.text.contains(FOX), "{name}: {}", r.text);
    }
}

#[test]
fn idle_between_characters() {
    let mut rng = Rng::new(4);
    let mut idle = || -2.0 * (1.0 - rng.uniform()).ln();
    let seg = synth::async_bits(
        &ita2_encode(&text(), true),
        5,
        Parity::None,
        1.0,
        &mut idle,
        4.0,
        4.0,
    );
    let r = analyze(
        &run(
            &seg,
            75.0,
            850.0,
            &Cond {
                snr_db: Some(10.0),
                ..Cond::new(0.0)
            },
            1,
        ),
        FS,
        None,
        0.0,
    );
    assert_eq!(r.params.as_ref().map(|p| p.framing()), Some("5N1".into()));
    assert!(r.text.contains("THE QUICK BROWN FOX"), "{}", r.text);
}

fn similarity(a: &str, b: &str) -> f64 {
    // 1 - normalised edit distance
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
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
    1.0 - d[b.len()] as f64 / a.len().max(b.len()).max(1) as f64
}

#[test]
fn weak() {
    let t = text();
    let seg = synth::async_simple(&ita2_encode(&t, true), 5, Parity::None, 1.5);
    let r = analyze(
        &run(
            &seg,
            45.45,
            170.0,
            &Cond {
                snr_db: Some(-5.0),
                ..Cond::new(-200.0)
            },
            7,
        ),
        FS,
        None,
        0.0,
    );
    assert!(is_valid(&r, 12), "{:?}", r.params);
    let p = r.params.as_ref().expect("params");
    assert_eq!((p.baud, p.framing().as_str()), (45.45, "5N1.5"));
    assert!(similarity(&r.text, &t) > 0.85, "{}", r.text);
}

#[test]
fn selective_fading() {
    let seg = synth::async_simple(&ita2_encode(&text(), true), 5, Parity::None, 1.5);
    let c = Cond {
        snr_db: Some(10.0),
        fade_hz: 0.3,
        selective: true,
        ..Cond::new(0.0)
    };
    let r = analyze(&run(&seg, 50.0, 450.0, &c, 3), FS, None, 0.0);
    let p = r.params.expect("params");
    assert_eq!(
        (p.baud, p.framing().as_str(), p.charset.as_str()),
        (50.0, "5N1.5", "ITA2")
    );
}

#[test]
fn sitor_b() {
    let seg = synth::sitor_bits(&text(), 8);
    let r = analyze(
        &run(
            &seg,
            100.0,
            170.0,
            &Cond {
                snr_db: Some(0.0),
                ..Cond::new(0.0)
            },
            2,
        ),
        FS,
        None,
        0.0,
    );
    let p = r.params.clone().expect("params");
    assert_eq!(
        (p.mode, p.charset.as_str(), p.baud),
        (Mode::Sitor, "CCIR476", 100.0)
    );
    assert!(
        r.text
            .contains("THE QUICK BROWN FOX JUMPS OVER THE LAZY DOG"),
        "{}",
        r.text
    );
}

#[test]
fn reversals_and_noise() {
    let seg: synth::Seg = (0..2000).map(|i| ((i & 1) as u8, 1.0)).collect();
    let r = analyze(
        &run(
            &seg,
            50.0,
            250.0,
            &Cond {
                snr_db: Some(10.0),
                ..Cond::new(0.0)
            },
            1,
        ),
        FS,
        None,
        0.0,
    );
    let p = r.params.clone().expect("params");
    assert_eq!((p.mode, p.baud), (Mode::Sync, 50.0));
    assert!(p.charset.contains("reversals"));
    assert!(!is_valid(&r, 12));
    let n = run(
        &vec![(1, 400.0)],
        45.45,
        170.0,
        &Cond {
            snr_db: Some(-60.0),
            ..Cond::new(0.0)
        },
        1,
    );
    assert!(!is_valid(&analyze(&n, FS, None, 0.0), 12));
}

/// 1.5 stop bits make half-bit intervals; the rate must not come out doubled.
#[test]
fn baud_with_half_bit_stops() {
    let seg = synth::async_simple(
        &ita2_encode(&"RYRYRYRYRY ".repeat(20), true),
        5,
        Parity::None,
        1.5,
    );
    let x = run(&seg, 50.0, 450.0, &Cond::new(0.0), 1);
    let (d, _, _) = ToneDemod::new(FS, -225.0, 225.0, FS / 450.0).run(&x);
    let (b, _) = estimate_baud(&transitions(&d, 0.35), FS, 20.0, 700.0);
    assert!((b - 50.0).abs() < 0.2, "{b}");
}

#[test]
fn usos_from_text() {
    let dwd = "WETTERMELDUNGEN VON DONNERSTAG, DEM 15.10.2026 15.10.26 15:00 UTC. SVINOY SUEDWEST 7 11 GRAD 1001 HPA STAVANGER SUEDWEST 4 REGEN 13 GRAD 1009 HPA";
    let ham = "CQ TEST DE N0CALL 599 001 KN TU 5NN 12 N0CALL QRZ CQ CQ DE N0CALL K";
    for (t, usos) in [(dwd, false), (ham, true)] {
        let codes = ita2_encode(t, usos);
        assert_eq!(charset::ita2_usos(&codes, false), usos, "{t}");
        let seg = synth::async_simple(&codes, 5, Parity::None, 1.5);
        let r = analyze(
            &run(
                &seg,
                50.0,
                450.0,
                &Cond {
                    snr_db: Some(10.0),
                    ..Cond::new(0.0)
                },
                1,
            ),
            FS,
            None,
            0.0,
        );
        assert_eq!(r.params.as_ref().map(|p| p.usos), Some(usos), "{t}");
        let tail: String = t.chars().skip(t.chars().count() - 40).collect();
        assert!(r.text.contains(&tail), "{}", r.text);
    }
}

#[test]
fn decoder_in_blocks() {
    let seg = synth::async_simple(&ita2_encode(&text(), true), 5, Parity::None, 1.5);
    let x = run(
        &seg,
        45.45,
        170.0,
        &Cond {
            snr_db: Some(5.0),
            ..Cond::new(0.0)
        },
        1,
    );
    let mut p = FskParams::new(-85.0, 85.0, 45.45);
    p.stop = 1.5;
    let whole: String = FskDecoder::new(FS, p.clone(), 0.0)
        .feed(&x)
        .iter()
        .map(|i| i.1.as_str())
        .collect();
    let mut dec = FskDecoder::new(FS, p, 0.0);
    let parts: String = x
        .chunks(777)
        .flat_map(|c| dec.feed(c))
        .map(|i| i.1)
        .collect();
    assert_eq!(parts, whole);
    assert!(parts.contains(FOX), "{parts}");
}

fn skim(audio: &[f32], band: (f64, f64)) -> (Skimmer, Vec<Event>) {
    let mut sk = Skimmer::new(band.0, band.1);
    let mut ev = Vec::new();
    for c in audio.chunks(1200) {
        sk.push(c);
        ev.extend(sk.poll());
    }
    (sk, ev)
}

fn texts(ev: &[Event]) -> std::collections::BTreeMap<i64, String> {
    let mut m = std::collections::BTreeMap::new();
    for e in ev {
        if let Event::Text { hz, text, .. } = e {
            m.entry((hz / 50.0).round() as i64 * 50)
                .or_insert_with(String::new)
                .push_str(text);
        }
    }
    m
}

/// Ham RTTY and an ASCII signal in one SSB passband: both found, worked
/// out and decoded.
#[test]
fn skimmer_two_signals_in_audio() {
    let mut rng = Rng::new(5);
    let t = text();
    let a = synth::audio(
        &synth::async_bits(
            &ita2_encode(&t, true),
            5,
            Parity::None,
            1.5,
            &mut || 0.0,
            60.0,
            4.0,
        ),
        45.45,
        170.0,
        AUDIO_RATE,
        &Cond::new(915.0),
        0.1,
        &mut rng,
    );
    let b = synth::audio(
        &synth::async_bits(&ascii(&t), 8, Parity::None, 1.0, &mut || 0.0, 100.0, 4.0),
        100.0,
        450.0,
        AUDIO_RATE,
        &Cond {
            mark_high: false,
            ..Cond::new(2000.0)
        },
        0.1,
        &mut rng,
    );
    let n = a.len().min(b.len());
    let mut x: Vec<f32> = (0..n).map(|i| a[i] + b[i]).collect();
    for v in &mut x {
        *v += (0.01 * rng.normal()) as f32;
    }
    let (sk, ev) = skim(&x, (200.0, 2800.0));
    let m = texts(&ev);
    assert_eq!(
        m.keys().copied().collect::<Vec<_>>(),
        vec![900, 2000],
        "{m:?}"
    );
    for v in m.values() {
        assert!(
            v.contains("THE QUICK BROWN FOX JUMPS OVER THE LAZY DOG"),
            "{v}"
        );
        // nothing decoded from the idle tone before the text
        assert!(v.starts_with("CQ CQ TEST"), "{v}");
    }
    let act = sk.active();
    let p0 = &act
        .iter()
        .find(|a| (a.hz - 915.0).abs() < 10.0)
        .expect("RTTY")
        .params;
    assert_eq!((p0.baud, p0.framing().as_str()), (45.45, "5N1.5"));
    let p1 = &act
        .iter()
        .find(|a| (a.hz - 2000.0).abs() < 10.0)
        .expect("ASCII")
        .params;
    assert_eq!(
        (
            p1.baud,
            p1.framing().as_str(),
            p1.charset.as_str(),
            p1.mark_high
        ),
        (100.0, "8N1", "ASCII", false)
    );
}

/// Reversals are identified (not decoded); noise gives nothing.
#[test]
fn skimmer_identifies_reversals() {
    let mut rng = Rng::new(6);
    let seg: synth::Seg = (0..1500).map(|i| ((i & 1) as u8, 1.0)).collect();
    let mut x = synth::audio(
        &seg,
        50.0,
        250.0,
        AUDIO_RATE,
        &Cond::new(1500.0),
        0.1,
        &mut rng,
    );
    for v in &mut x {
        *v += (0.01 * rng.normal()) as f32;
    }
    let (sk, ev) = skim(&x, (200.0, 2800.0));
    assert!(texts(&ev).is_empty());
    let act = sk.active();
    assert_eq!(act.len(), 1, "{act:?}");
    assert!(!act[0].locked && act[0].params.charset.contains("reversals"));
    let noise: Vec<f32> = (0..(20.0 * AUDIO_RATE) as usize)
        .map(|_| (0.05 * rng.normal()) as f32)
        .collect();
    let (sk, ev) = skim(&noise, (200.0, 2800.0));
    assert!(ev.is_empty() && sk.active().is_empty());
}

/// Ham RTTY and an ASCII signal 19 kHz apart in a 60 kS/s channelizer
/// bin: both found, worked out and decoded.
#[test]
fn skimmer_wide_iq() {
    let rate = 60_000.0;
    let mut rng = Rng::new(31);
    let t = text();
    let a = synth::complex(
        &synth::async_bits(
            &ita2_encode(&t, true),
            5,
            Parity::None,
            1.5,
            &mut || 0.0,
            60.0,
            4.0,
        ),
        45.45,
        170.0,
        rate,
        &Cond::new(-12_000.0),
        &mut rng,
    );
    let b = synth::complex(
        &synth::async_bits(&ascii(&t), 8, Parity::None, 1.0, &mut || 0.0, 100.0, 4.0),
        100.0,
        450.0,
        rate,
        &Cond {
            mark_high: false,
            ..Cond::new(7_000.0)
        },
        &mut rng,
    );
    let n = a.len().min(b.len());
    let x: Vec<num_complex::Complex32> = (0..n)
        .map(|i| {
            (a[i] + b[i]) * 0.1
                + num_complex::Complex32::new(
                    (0.003 * rng.normal()) as f32,
                    (0.003 * rng.normal()) as f32,
                )
        })
        .collect();
    let mut sk = Skimmer::new_iq(rate, -20_000.0, 20_000.0);
    let mut ev = Vec::new();
    for c in x.chunks(2048) {
        sk.push_iq(c);
        ev.extend(sk.poll());
    }
    let m = texts(&ev);
    let keys: Vec<i64> = m.keys().copied().collect();
    assert_eq!(keys, vec![-12_000, 7_000], "{m:?}");
    for v in m.values() {
        assert!(
            v.contains("THE QUICK BROWN FOX JUMPS OVER THE LAZY DOG"),
            "{v}"
        );
    }
}

/// A contest station: short overs with pauses between. The first look at
/// it is mostly pause; it is still worked out (from the stretch it is on)
/// and its overs decoded, not ignored for minutes.
#[test]
fn skimmer_overs_with_pauses() {
    let mut rng = Rng::new(11);
    let over = "CQ TEST N0CALL N0CALL TEST\n";
    let rate = AUDIO_RATE;
    // 17 s quiet (the skimmer first looks at 18 s, with 1 s of the over
    // at the end of its 8 s: the analysis gives nothing; until 2026-11-16
    // the frequency was then ignored for 2 minutes and none was decoded),
    // then overs of ~6 s every 14 s
    let mut x = vec![0f32; (17.0 * rate) as usize];
    for _ in 0..6 {
        let seg = synth::async_bits(
            &ita2_encode(over, true),
            5,
            Parity::None,
            1.5,
            &mut || 0.0,
            8.0,
            2.0,
        );
        let a = synth::audio(&seg, 45.45, 170.0, rate, &Cond::new(1500.0), 0.1, &mut rng);
        let on = a.len();
        x.extend(a);
        x.extend(std::iter::repeat_n(0.0, (14.0 * rate) as usize - on));
    }
    for v in &mut x {
        *v += (0.01 * rng.normal()) as f32;
    }
    let (_, ev) = skim(&x, (200.0, 2800.0));
    let all: String = texts(&ev).values().cloned().collect();
    assert!(all.matches("N0CALL N0CALL").count() >= 5, "{all:?}");
}

/// DSC (ITU-R M.493): calls from a coast station, 100 Bd 170 Hz with B (0)
/// the upper tone, a few seconds apart, found in the passband and read.
#[test]
fn skimmer_reads_dsc_calls() {
    use web888_fsk::dsc;
    let mut rng = Rng::new(21);
    let rate = AUDIO_RATE;
    // individual call to ship 123456780 from coast 001234560, routine,
    // J3E TP, 8291.0 kHz both ways, ack RQ
    let call = [
        120, 12, 34, 56, 78, 0, 100, 0, 12, 34, 56, 0, 109, 126, 8, 29, 10, 8, 29, 10, 117,
    ];
    let mut x = vec![0f32; (4.0 * rate) as usize];
    for _ in 0..4 {
        let seg: synth::Seg = dsc::call_bits(&call, 200)
            .into_iter()
            .map(|b| (b, 1.0))
            .collect();
        let mut c = Cond::new(1700.0);
        c.mark_high = false;
        x.extend(synth::audio(&seg, 100.0, 170.0, rate, &c, 0.1, &mut rng));
        x.extend(std::iter::repeat_n(0.0, (5.0 * rate) as usize));
    }
    for v in &mut x {
        *v += (0.01 * rng.normal()) as f32;
    }
    let (_, ev) = skim(&x, (200.0, 2800.0));
    let all: String = texts(&ev).values().cloned().collect();
    let want = "DSC individual | routine | to 123456780 (ship) | from 001234560 (coast) | J3E TP | RX 8291.0 kHz TX 8291.0 kHz | ack RQ | ECC ok";
    assert!(all.matches(want).count() >= 3, "{all:?}");
    assert!(
        ev.iter()
            .any(|e| matches!(e, Event::Lock { params, .. } if params.mode == Mode::Dsc))
    );
}
