// SPDX-License-Identifier: GPL-3.0-or-later
//! The FSK/RTTY skimmer (web888-fsk) for the web UI: built for
//! wasm32-unknown-unknown (`scripts/build-wasm.sh`) and run in a Web
//! Worker (`ui/src/rtty/worker.ts`). A plain C ABI, one skimmer per
//! instance:
//!
//! * `fsk_buffer(n)`: room for n samples (12 kHz mono f32)
//! * `fsk_push(n)`: runs them through; returns the length of the JSON at
//!   `fsk_output()` when there is something new (text, or a run every 2
//!   s), else 0: `{"ev":[...],"act":[...],"now":..}` with events
//!   `{"k":"lock","id","hz","p"}`, `{"k":"text","id","hz","text","t"}`,
//!   `{"k":"ident","hz","p"}`, signals `{"id","hz","locked","snr","fe","p"}`
//!   and parameters `p` = `{"baud","shift","lo","hi","markHigh","framing",
//!   "charset","usos","msb","mode"}` (audio Hz; times in s since the reset)
//! * `fsk_band(lo, hi)`: the audio band searched (Hz)
//! * `fsk_reset()`: a new stream (after retuning)

use std::cell::RefCell;
use std::fmt::Write;
use web888_fsk::analyze::Mode;
use web888_fsk::{Active, Event, FskParams, Skimmer};

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

fn params(s: &mut String, p: &FskParams) {
    let mode = match p.mode {
        Mode::Async => "async",
        Mode::Sitor => "sitor",
        Mode::Sync => "sync",
    };
    let _ = write!(
        s,
        "{{\"baud\":{},\"shift\":{:.1},\"lo\":{:.1},\"hi\":{:.1},\"markHigh\":{},\"framing\":",
        p.baud,
        p.shift(),
        p.f_lo,
        p.f_hi,
        p.mark_high
    );
    esc(s, &p.framing());
    s.push_str(",\"charset\":");
    esc(s, &p.charset);
    let _ = write!(
        s,
        ",\"usos\":{},\"msb\":{},\"mode\":\"{mode}\"}}",
        p.usos, p.msb_first
    );
}

/// The JSON of what's new.
pub fn to_json(ev: &[Event], act: &[Active], now: f64) -> String {
    let mut s = String::from("{\"ev\":[");
    for (i, e) in ev.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        match e {
            Event::Lock { id, hz, params: p } => {
                let _ = write!(s, "{{\"k\":\"lock\",\"id\":{id},\"hz\":{hz:.1},\"p\":");
                params(&mut s, p);
                s.push('}');
            }
            Event::Text { id, hz, text, t } => {
                let _ = write!(
                    s,
                    "{{\"k\":\"text\",\"id\":{id},\"hz\":{hz:.1},\"t\":{t:.3},\"text\":"
                );
                esc(&mut s, text);
                s.push('}');
            }
            Event::Ident { hz, params: p } => {
                let _ = write!(s, "{{\"k\":\"ident\",\"hz\":{hz:.1},\"p\":");
                params(&mut s, p);
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
            "{{\"id\":{},\"hz\":{:.1},\"locked\":{},\"snr\":{:.1},\"fe\":{:.3},\"p\":",
            a.id,
            a.hz,
            a.locked,
            a.snr.max(-99.0),
            a.fe
        );
        params(&mut s, &a.params);
        s.push('}');
    }
    let _ = write!(s, "],\"now\":{now:.3}}}");
    s
}

#[unsafe(no_mangle)]
pub extern "C" fn fsk_buffer(n: usize) -> *mut f32 {
    STATE.with_borrow_mut(|s| {
        s.input.resize(n, 0.0);
        s.input.as_mut_ptr()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn fsk_push(n: usize) -> usize {
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
pub extern "C" fn fsk_output() -> *const u8 {
    STATE.with_borrow(|s| s.output.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn fsk_band(lo: f64, hi: f64) {
    STATE.with_borrow_mut(|s| {
        if (lo, hi) != s.band {
            s.band = (lo, hi);
            s.skimmer.set_band(lo, hi);
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn fsk_reset() {
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
        let mut p = FskParams::new(-85.0, 85.0, 45.45);
        p.mark_high = false;
        let ev = vec![
            Event::Lock {
                id: 2,
                hz: 915.0,
                params: p.clone(),
            },
            Event::Text {
                id: 2,
                hz: 915.0,
                text: "CQ \"X\"\nDE".into(),
                t: 1.5,
            },
            Event::Ident {
                hz: 1500.0,
                params: FskParams {
                    charset: "reversals (idle)".into(),
                    mode: Mode::Sync,
                    ..FskParams::new(-125.0, 125.0, 50.0)
                },
            },
        ];
        let act = vec![Active {
            id: 2,
            hz: 915.0,
            params: p,
            snr: 7.04,
            fe: 0.012,
            locked: true,
        }];
        let j = to_json(&ev, &act, 9.0);
        let p = r#"{"baud":45.45,"shift":170.0,"lo":-85.0,"hi":85.0,"markHigh":false,"framing":"5N1.5","charset":"ITA2","usos":true,"msb":false,"mode":"async"}"#;
        assert!(j.starts_with(&format!(r#"{{"ev":[{{"k":"lock","id":2,"hz":915.0,"p":{p}}},{{"k":"text","id":2,"hz":915.0,"t":1.500,"text":"CQ \"X\"\nDE"}},{{"k":"ident","hz":1500.0,"p":{{"baud":50,"#)), "{j}");
        assert!(
            j.contains(r#""framing":"sync","charset":"reversals (idle)""#),
            "{j}"
        );
        assert!(j.ends_with(&format!(r#"],"act":[{{"id":2,"hz":915.0,"locked":true,"snr":7.0,"fe":0.012,"p":{p}}}],"now":9.000}}"#)), "{j}");
    }
}
