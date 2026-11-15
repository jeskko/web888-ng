// SPDX-License-Identifier: GPL-3.0-or-later
//! Word spaces: the neural decoder's raw output on synthetic CW, with the
//! spaces counted (the character error rate ignores them), and through the
//! skimmer.
//!
//!     cargo run --release -p web888-cw --example spaces

use web888_cw::dsp::Rng;
use web888_cw::synth::{self, Channel, Keying};
use web888_cw::{AUDIO_RATE, ENV_FS, Skimmer, net};

const MSGS: [&str; 4] = [
    "CQ CQ DE N0CALL N0CALL K",
    "UR RST 579 579 NAME JOHN QTH OULU",
    "TNX FER QSO 73 ES GL DE N1CALL",
    "RIG IC7300 PWR 100W ANT DIPOLE",
];

/// (spaces right, spaces wanted, spaces extra) over word boundaries.
fn spaces(want: &str, got: &str) -> (usize, usize, usize) {
    let w: Vec<&str> = want.split(' ').collect();
    let g: Vec<&str> = got.split(' ').filter(|s| !s.is_empty()).collect();
    let right = w
        .windows(2)
        .filter(|p| {
            g.windows(2)
                .any(|q| q[0].ends_with(p[0]) && q[1].starts_with(p[1]))
        })
        .count();
    (right, w.len() - 1, g.len().saturating_sub(w.len()))
}

fn main() {
    for wpm in [15.0, 25.0, 35.0] {
        for jitter in [0.0, 0.1] {
            let (mut r, mut n, mut extra) = (0, 0, 0);
            for (i, m) in MSGS.iter().enumerate() {
                let k = Keying {
                    jitter,
                    ..Keying::new(wpm)
                };
                let x = synth::complex(
                    m,
                    &k,
                    &Channel {
                        snr_db: Some(6.0),
                        ..Channel::new(0.0)
                    },
                    ENV_FS,
                    &mut Rng::new(i as u64),
                );
                let t: String = net::decode(&x, 0.0).into_iter().map(|c| c.1).collect();
                let (a, b, c) = spaces(m, &t);
                r += a;
                n += b;
                extra += c;
                if wpm == 25.0 && jitter == 0.0 {
                    println!("  {t:?}");
                }
            }
            println!("raw {wpm} WPM jitter {jitter}: spaces {r}/{n}, extra {extra}");
        }
    }
    // through the skimmer (audio)
    let mut rng = Rng::new(3);
    let text = MSGS.join(" ");
    let k = Keying {
        lead: 0.5,
        tail: 2.0,
        ..Keying::new(25.0)
    };
    let s = synth::audio(&text, &k, &Channel::new(700.0), AUDIO_RATE, 0.1, &mut rng);
    let mut a = synth::audio_noise(s.len() * 2, AUDIO_RATE, 0.1, 6.0, &mut rng);
    for (i, v) in a.iter_mut().enumerate() {
        *v += s[i % s.len()];
    }
    let mut sk = Skimmer::new(200.0, 2800.0);
    sk.neural = true;
    let mut out = String::new();
    for c in a.chunks(1024) {
        sk.push(c);
        for e in sk.poll() {
            out.push_str(&e.text);
        }
    }
    println!("skimmer: {out:?}");
}
