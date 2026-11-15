// SPDX-License-Identifier: GPL-3.0-or-later
//! The PSK31/63/125 skimmer (web888-psk) for the web UI: built for
//! wasm32-unknown-unknown (`scripts/build-wasm.sh`) and run in a Web
//! Worker (`ui/src/psk/worker.ts`). A plain C ABI, one skimmer per
//! instance:
//!
//! * `psk_buffer(n)`: room for n samples (12 kHz mono f32)
//! * `psk_push(n)`: runs them through; returns the length of the JSON at
//!   `psk_output()` when there is something new (text, or a run every 2
//!   s), else 0: `{"ev":[...],"act":[...],"now":..}` with events
//!   `{"k":"lock","id","hz","baud"}`, `{"k":"text","id","hz","t","text"}`
//!   (and `"weak"`: a '0'/'1' per character, '1' where it may have read
//!   otherwise, when any is), and signals `{"id","hz","baud","q","open"}`
//!   (audio Hz; times in s since the reset)
//! * `psk_band(lo, hi)`: the audio band searched (Hz)
//! * `psk_reset()`: a new stream (after retuning)

use std::cell::RefCell;
use std::fmt::Write;
use web888_psk::{Active, Event, Skimmer};

struct State {
    skimmer: Skimmer,
    band: (f64, f64),
    runs: u64,
    input: Vec<f32>,
    output: String,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        skimmer: Skimmer::default(),
        band: (200.0, 2800.0),
        runs: 0,
        input: Vec::new(),
        output: String::new(),
    });
}

fn esc(s: &mut String, t: &str) {
    s.push('"');
    for c in t.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            c if (c as u32) < 0x20 => {
                let _ = write!(s, "\\u{:04x}", c as u32);
            }
            c => s.push(c),
        }
    }
    s.push('"');
}

/// The JSON of what's new.
pub fn to_json(ev: &[Event], act: &[Active], now: f64) -> String {
    let mut s = String::from("{\"ev\":[");
    for (i, e) in ev.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        match e {
            Event::Lock { id, hz, baud } => {
                let _ = write!(
                    s,
                    "{{\"k\":\"lock\",\"id\":{id},\"hz\":{hz:.1},\"baud\":{baud}}}"
                );
            }
            Event::Text {
                id,
                hz,
                text,
                t,
                weak,
            } => {
                let _ = write!(
                    s,
                    "{{\"k\":\"text\",\"id\":{id},\"hz\":{hz:.1},\"t\":{t:.3},\"text\":"
                );
                esc(&mut s, text);
                if weak.iter().any(|&w| w) {
                    s.push_str(",\"weak\":\"");
                    s.extend(weak.iter().map(|&w| if w { '1' } else { '0' }));
                    s.push('"');
                }
                s.push('}');
            }
        }
    }
    s.push_str("],\"act\":[");
    for (i, a) in act.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(
            s,
            "{{\"id\":{},\"hz\":{:.1},\"baud\":{},\"q\":{:.2},\"open\":{}}}",
            a.id, a.hz, a.baud, a.quality, a.open
        );
    }
    let _ = write!(s, "],\"now\":{now:.3}}}");
    s
}

#[unsafe(no_mangle)]
pub extern "C" fn psk_buffer(n: usize) -> *mut f32 {
    STATE.with_borrow_mut(|s| {
        s.input.resize(n, 0.0);
        s.input.as_mut_ptr()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn psk_push(n: usize) -> usize {
    STATE.with_borrow_mut(|s| {
        let n = n.min(s.input.len());
        let State { skimmer, input, .. } = s;
        skimmer.push(&input[..n]);
        let ev = s.skimmer.poll();
        if ev.is_empty() && s.skimmer.runs == s.runs {
            return 0;
        }
        s.runs = s.skimmer.runs;
        s.output = to_json(&ev, &s.skimmer.active(), s.skimmer.now());
        s.output.len()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn psk_output() -> *const u8 {
    STATE.with_borrow(|s| s.output.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn psk_band(lo: f64, hi: f64) {
    STATE.with_borrow_mut(|s| {
        if (lo, hi) != s.band {
            s.band = (lo, hi);
            s.skimmer.set_band(lo, hi);
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn psk_reset() {
    STATE.with_borrow_mut(|s| {
        s.skimmer = Skimmer::new(s.band.0, s.band.1);
        s.runs = 0;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json() {
        let ev = vec![
            Event::Lock {
                id: 1,
                hz: 1000.0,
                baud: 31.25,
            },
            Event::Text {
                id: 1,
                hz: 1000.25,
                text: "CQ \"de\"\n".into(),
                t: 2.5,
                weak: vec![false; 8],
            },
            Event::Text {
                id: 1,
                hz: 1000.25,
                text: "K".into(),
                t: 3.0,
                weak: vec![true],
            },
        ];
        let act = vec![Active {
            id: 1,
            hz: 1000.25,
            baud: 31.25,
            quality: 0.951,
            open: true,
        }];
        assert_eq!(
            to_json(&ev, &act, 4.0),
            r#"{"ev":[{"k":"lock","id":1,"hz":1000.0,"baud":31.25},{"k":"text","id":1,"hz":1000.2,"t":2.500,"text":"CQ \"de\"\n"},{"k":"text","id":1,"hz":1000.2,"t":3.000,"text":"K","weak":"1"}],"act":[{"id":1,"hz":1000.2,"baud":31.25,"q":0.95,"open":true}],"now":4.000}"#
        );
    }
}
