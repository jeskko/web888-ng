// SPDX-License-Identifier: GPL-3.0-or-later
//! Decoding errors on a recording, one at a time: each suspected error
//! (from a spellcheck of the skimmer's text) is decoded again from the IQ
//! with variants of the receiver, to see which (if any) reads it right.
//! Words read right as skimmed (a control set) are decoded the same way,
//! so a variant that breaks good text shows too.
//!
//!     cargo run --release -p web888-fsk --example dive FILE.cf32 RATE CENTRE_HZ ERRORS [GOOD]
//!
//! ERRORS, GOOD: lines `t0 t1 kHz id got want` (stream seconds, RF kHz of
//! the signal's centre). BITS=1 prints each character's soft bits.
//!
//! The errors are the baseline's by selection, so any variant reads
//! some of them right by chance: compare receivers with `spans`.

use num_complex::{Complex32, Complex64};
use std::io::{Read, Seek, SeekFrom};
use web888_cw::dsp;
use web888_fsk::analyze::{FskDecoder, FskParams, Scan, async_scan, refine_tones};
use web888_fsk::charset::Ita2Auto;

const FS: f64 = 4000.0;
const BAUD: f64 = 45.45;

/// x (at rate) mixed down from hz, filtered to ±half_bw by `taps`-long
/// FIR and decimated to FS. Returns the output and its delay (s).
fn channel(
    x: &[Complex32],
    rate: f64,
    hz: f64,
    half_bw: f64,
    taps: usize,
) -> (Vec<Complex32>, f64) {
    let decim = (rate / FS).round() as usize;
    let n = taps | 1;
    let h = dsp::firwin(n, half_bw, rate);
    let w = -2.0 * std::f64::consts::PI * hz / rate;
    let y: Vec<Complex32> = x
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let p = Complex64::from_polar(1.0, w * i as f64);
            v * Complex32::new(p.re as f32, p.im as f32)
        })
        .collect();
    let out = (0..(y.len().saturating_sub(n)) / decim)
        .map(|k| {
            let s = &y[k * decim..k * decim + n];
            s.iter().zip(&h).map(|(a, &h)| a * h).sum()
        })
        .collect();
    (out, (n - 1) as f64 / 2.0 / rate)
}

fn read(path: &str, rate: f64, t0: f64, t1: f64) -> Vec<Complex32> {
    let mut f = std::fs::File::open(path).expect("open");
    let i0 = (t0.max(0.0) * rate) as u64;
    let n = ((t1 - t0.max(0.0)) * rate) as usize;
    f.seek(SeekFrom::Start(8 * i0)).expect("seek");
    let mut b = vec![0u8; 8 * n];
    let mut got = 0;
    while got < b.len() {
        match f.read(&mut b[got..]) {
            Ok(0) | Err(_) => break,
            Ok(k) => got += k,
        }
    }
    b[..got - got % 8]
        .as_chunks::<8>()
        .0
        .iter()
        .map(|c| {
            Complex32::new(
                f32::from_le_bytes(c[..4].try_into().unwrap()),
                f32::from_le_bytes(c[4..].try_into().unwrap()),
            )
        })
        .collect()
}

/// Tone filter of a variant.
#[derive(Clone, Copy)]
enum Tone {
    /// 1-bit boxcar (the decoder's ToneDemod).
    Boxcar,
    /// Hann window `len` bits long.
    Hann(f64),
    /// Raised-cosine edged boxcar: `len` bits, cosine edges of `edge` bits each.
    Tukey(f64, f64),
}

/// Envelopes of the two tones through filter `tone`, causal, delayed so that a bit's
/// clean reading lands at its end (as the boxcar's).
fn envelopes(y: &[Complex32], f: [f64; 2], tone: Tone) -> (Vec<f64>, Vec<f64>) {
    let t = FS / BAUD;
    let h: Vec<f64> = match tone {
        Tone::Boxcar => vec![1.0; t.round() as usize],
        Tone::Hann(len) => {
            let m = (len * t).round() as usize;
            (0..m)
                .map(|i| {
                    0.5 - 0.5 * (2.0 * std::f64::consts::PI * (i as f64 + 0.5) / m as f64).cos()
                })
                .collect()
        }
        Tone::Tukey(len, edge) => {
            let m = (len * t).round() as usize;
            let e = (edge * t).max(1.0);
            (0..m)
                .map(|i| {
                    let x = (i as f64 + 0.5).min(m as f64 - i as f64 - 0.5);
                    if x >= e {
                        1.0
                    } else {
                        0.5 - 0.5 * (std::f64::consts::PI * x / e).cos()
                    }
                })
                .collect()
        }
    };
    let s: f64 = h.iter().sum();
    let m = h.len();
    // align: the boxcar's centre is L/2 back from its output; keep that
    let shift = (m as f64 - t.round()) / 2.0;
    let sh = shift.round() as isize;
    let mut out = [Vec::with_capacity(y.len()), Vec::with_capacity(y.len())];
    for k in 0..2 {
        let w = -2.0 * std::f64::consts::PI * f[k] / FS;
        let z: Vec<Complex64> = y
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                Complex64::new(v.re as f64, v.im as f64) * Complex64::from_polar(1.0, w * i as f64)
            })
            .collect();
        for i in 0..y.len() {
            // output i: filter centred at i - L/2 (as the boxcar), i.e. taps end at i + sh
            let end = i as isize + sh;
            let mut acc = Complex64::new(0.0, 0.0);
            for j in 0..m {
                let q = end - j as isize;
                if q >= 0 && (q as usize) < z.len() {
                    acc += z[q as usize] * h[m - 1 - j];
                }
            }
            out[k].push((acc / s).norm());
        }
    }
    let [a, b] = out;
    (a, b)
}

/// ToneDemod's normalisation (automatic threshold), on given envelopes.
fn atc(env: &(Vec<f64>, Vec<f64>)) -> Vec<f32> {
    let al = 1.0 / (0.6 * FS);
    let u = |a: f64, b: f64| {
        let on = if b > a { 1.0 } else { 0.0 };
        [
            a * (1.0 - on),
            b * on,
            1.0 - on,
            on,
            a * on,
            b * (1.0 - on),
            on,
            1.0 - on,
        ]
    };
    let n = env.0.len();
    let m = n.min((0.3 * FS) as usize).max(1);
    let mut y = [0.0; 8];
    for i in 0..m.min(n) {
        let ui = u(env.0[i], env.1[i]);
        for k in 0..8 {
            y[k] += ui[k];
        }
    }
    let mut y = y.map(|v| v / m as f64 / (1.0 - al) + 1e-12);
    let mut d = Vec::with_capacity(n);
    for i in 0..n {
        let (a, b) = (env.0[i], env.1[i]);
        let ui = u(a, b);
        for k in 0..8 {
            y[k] = al * ui[k] + (1.0 - al) * y[k];
        }
        let mut la = y[0] / y[2].max(1e-6);
        let mut lb = y[1] / y[3].max(1e-6);
        let mut na = y[4] / y[6].max(1e-6);
        let mut nb = y[5] / y[7].max(1e-6);
        if y[2] < 0.02 {
            (la, nb) = (lb, na);
        } else if y[3] < 0.02 {
            (lb, na) = (la, nb);
        }
        let da = (la - na).max(0.15 * la + 1e-12);
        let db = (lb - nb).max(0.15 * lb + 1e-12);
        d.push(((b - nb) / db - (a - na) / da) as f32);
    }
    d
}

/// Characters (start time, char, soft bits) of d.
fn chars(d: &[f32], a: &[f32], b: &[f32], ty: f64) -> Vec<(f64, String, Vec<f32>)> {
    let t = FS / BAUD;
    let (cs, _, _) = async_scan(
        d,
        a,
        b,
        t,
        5,
        &Scan {
            stop: Some(1.5),
            ..Scan::default()
        },
    );
    let mut ita = Ita2Auto::new(true, false);
    cs.into_iter()
        .map(|c| {
            let v = (0..=6)
                .map(|k| d[((c.t + (k as f64 + 0.5) * t).round() as usize).min(d.len() - 1)])
                .collect();
            let s = ita.char(c.code & 31).map(String::from).unwrap_or_default();
            (ty + (c.t - 0.5 * t) / FS, s, v)
        })
        .collect()
}

fn words_in(cs: &[(f64, String, Vec<f32>)], t0: f64, t1: f64) -> String {
    cs.iter()
        .filter(|c| c.0 >= t0 - 0.25 && c.0 <= t1 + 0.1)
        .map(|c| c.1.as_str())
        .collect()
}

/// The smallest |soft bit| (start, data, stop) of the character where
/// `got` differs from `want` (same length), in the decoded word `got`.
fn margin(cs: &[(f64, String, Vec<f32>)], t0: f64, t1: f64, got: &str, want: &str) -> Option<f32> {
    let g: Vec<char> = got.chars().collect();
    let w: Vec<char> = want.chars().collect();
    if g.len() != w.len() {
        return None;
    }
    let j = (0..g.len()).find(|&k| g[k] != w[k])?;
    let win: Vec<&(f64, String, Vec<f32>)> = cs
        .iter()
        .filter(|c| c.0 >= t0 - 0.25 && c.0 <= t1 + 0.1 && !c.1.is_empty())
        .collect();
    for i in 0..win.len() {
        if i + g.len() <= win.len() && (0..g.len()).all(|k| win[i + k].1 == g[k].to_string()) {
            let c = win[i + j];
            return Some(c.2.iter().map(|v| v.abs()).fold(f32::INFINITY, f32::min));
        }
    }
    None
}

struct Variant {
    name: &'static str,
    sharp: bool,
    afc: bool,
    tone: Tone,
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let rate: f64 = a[2].parse().expect("rate");
    let centre: f64 = a[3].parse().expect("centre");
    let shift = 170.0;
    let vs = [
        Variant {
            name: "as skimmed (boxcar 1 bit)",
            sharp: false,
            afc: false,
            tone: Tone::Boxcar,
        },
        Variant {
            name: "tones re-measured (AFC)",
            sharp: false,
            afc: true,
            tone: Tone::Boxcar,
        },
        Variant {
            name: "sharp channel ±200 Hz",
            sharp: true,
            afc: false,
            tone: Tone::Boxcar,
        },
        Variant {
            name: "Hann 1.5 bits",
            sharp: false,
            afc: false,
            tone: Tone::Hann(1.5),
        },
        Variant {
            name: "Hann 2 bits",
            sharp: false,
            afc: false,
            tone: Tone::Hann(2.0),
        },
        Variant {
            name: "Tukey 1.2 bits, 0.4 edges",
            sharp: false,
            afc: false,
            tone: Tone::Tukey(1.2, 0.4),
        },
        Variant {
            name: "sharp + Hann 1.5",
            sharp: true,
            afc: false,
            tone: Tone::Hann(1.5),
        },
        Variant {
            name: "sharp + AFC + Hann 1.5",
            sharp: true,
            afc: true,
            tone: Tone::Hann(1.5),
        },
    ];
    let bits = std::env::var("BITS").is_ok();
    for (set, path) in [("errors", a.get(4)), ("good", a.get(5))] {
        let Some(path) = path else { continue };
        let mut wins = vec![0usize; vs.len()];
        let mut n = 0usize;
        let mut first_or = 0usize;
        let mut dec_check = 0usize;
        let mut classes = std::collections::BTreeMap::new();
        for line in std::fs::read_to_string(path).expect("list").lines() {
            let v: Vec<&str> = line.split_whitespace().collect();
            if v.len() < 6 {
                continue;
            }
            let (t0, t1): (f64, f64) = (v[0].parse().unwrap(), v[1].parse().unwrap());
            let khz: f64 = v[2].parse().unwrap();
            let (got, want) = (v[4], v[5]);
            n += 1;
            let ts = (t0 - 8.0).max(0.0);
            let x = read(&a[1], rate, ts, t1 + 3.0);
            let hz = khz * 1e3 - centre;
            let (wide, dw) = channel(
                &x,
                rate,
                hz,
                0.5 * shift + 250.0,
                (97.0 * rate / 12_000.0) as usize,
            );
            let (sharp, ds) = channel(
                &x,
                rate,
                hz,
                0.5 * shift + 115.0,
                (1200.0 * rate / 40_000.0) as usize,
            );
            // the stream decoder on the same input: is the error reproduced?
            let mut dec =
                FskDecoder::new(FS, FskParams::new(-0.5 * shift, 0.5 * shift, BAUD), ts - dw);
            let s0: String = dec
                .feed(&wide)
                .into_iter()
                .filter(|it| it.0 >= t0 - 0.25 && it.0 <= t1 + 0.1)
                .map(|it| it.1)
                .collect();
            dec_check += usize::from(s0.split_whitespace().any(|w| w == got));
            let k0 = ((t0 - 1.5 - ts) * FS).max(0.0) as usize;
            let k1 = (((t1 + 1.5 - ts) * FS) as usize).min(wide.len());
            let (lo, hi) = refine_tones(&wide[k0..k1], FS, -0.5 * shift, 0.5 * shift);
            let mut row = Vec::new();
            let mut mg = None;
            let mut fixed_by_other = false;
            for (k, var) in vs.iter().enumerate() {
                let (y, dl) = if var.sharp { (&sharp, ds) } else { (&wide, dw) };
                let f = if var.afc {
                    [lo, hi]
                } else {
                    [-0.5 * shift, 0.5 * shift]
                };
                let env = envelopes(y, f, var.tone);
                let d = atc(&env);
                let af: Vec<f32> = env.0.iter().map(|&v| v as f32).collect();
                let bf: Vec<f32> = env.1.iter().map(|&v| v as f32).collect();
                let cs = chars(&d, &af, &bf, ts - dl);
                if bits && k == 0 {
                    for c in cs.iter().filter(|c| c.0 >= t0 - 0.4 && c.0 <= t1 + 0.2) {
                        let v: Vec<String> = c.2.iter().map(|x| format!("{x:+5.2}")).collect();
                        println!("    {:7.2} {:?} [{}]", c.0, c.1, v.join(" "));
                    }
                }
                if k == 0 {
                    mg = margin(&cs, t0, t1, got, want);
                }
                let s = words_in(&cs, t0, t1);
                let ok = s.split_whitespace().any(|w| w == want);
                if ok {
                    wins[k] += 1;
                    fixed_by_other |= k > 0;
                }
                row.push(format!(
                    "{}{}",
                    if ok { "+" } else { " " },
                    s.trim().replace(['\r', '\n'], " ")
                ));
            }
            first_or += usize::from(fixed_by_other && !row[0].starts_with('+'));
            let all_same = row.iter().all(|r| r.trim() == row[0].trim());
            let class = match mg {
                _ if row[0].starts_with('+') => "-",
                _ if fixed_by_other => "fixable",
                Some(m) if m >= 0.5 && all_same => "as-sent",
                Some(_) => "marginal",
                None => "other",
            };
            *classes.entry(class).or_insert(0usize) += 1;
            if set == "errors"
                || !row[0].starts_with('+')
                || row.iter().any(|r| !r.starts_with('+'))
            {
                println!(
                    "{set:6} {t0:7.2} {khz:9.3} {got:>10} -> {want:<10} {class:8} {:4.2} afc {lo:+6.1} {hi:+6.1} | {}",
                    mg.unwrap_or(f32::NAN),
                    row.join(" | ")
                );
            }
        }
        println!("\n{set}: {n} words; the stream decoder reads {dec_check} as listed");
        for (k, var) in vs.iter().enumerate() {
            println!("{:>5} right by {}", wins[k], var.name);
        }
        println!("{first_or:>5} wrong by the first and right by some other");
        println!("classes {classes:?}\n");
    }
}
