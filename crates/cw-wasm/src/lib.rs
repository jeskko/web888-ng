// SPDX-License-Identifier: GPL-3.0-or-later
//! The CW skimmer (web888-cw) for the web UI: built for
//! wasm32-unknown-unknown (`scripts/build-wasm.sh`) and run in a Web
//! Worker (`ui/src/cw/worker.ts`). A plain C ABI, one skimmer per
//! instance:
//!
//! * `cw_buffer(n)`: room for n samples (12 kHz mono f32); returns where
//!   the caller writes them
//! * `cw_push(n)`: runs them through; after a decode run (every 2 s)
//!   returns the length of the JSON at `cw_output()`, else 0:
//!   `{"ev":[{"id":..,"hz":..,"text":..,"wpm":..,"snr":..,"t":..}],
//!   "act":[{"id":..,"hz":..,"wpm":..,"snr":..}],"now":..}` (times in s
//!   since the last reset)
//! * `cw_band(lo, hi)`: the audio band searched (Hz)
//! * `cw_neural(on)`: text from the neural decoder (1) or the classical
//! * `cw_wide(rate, lo, hi)`: complex IQ at `rate` (a channelizer bin),
//!   signals between `lo` and `hi` Hz from its centre; `rate` 0: back to
//!   audio. `cw_buffer_iq(n)` / `cw_push_iq(n)` take n complex samples
//!   (I, Q interleaved f32) as `cw_buffer` / `cw_push` take audio
//! * `cw_reset()`: a new stream (after retuning)

use std::cell::RefCell;
use std::fmt::Write;
use web888_cw::{Active, Event, Skimmer};

struct State {
    skimmer: Skimmer,
    /// IQ input: (rate, lo, hi).
    wide: Option<(f64, f64, f64)>,
    input_iq: Vec<f32>,
    neural: bool,
    band: (f64, f64),
    runs: u64,
    input: Vec<f32>,
    output: String,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        skimmer: Skimmer::default(),
        wide: None,
        input_iq: Vec::new(),
        neural: false,
        band: (200.0, 2800.0),
        runs: 0,
        input: Vec::new(),
        output: String::new(),
    });
}

fn esc(s: &mut String, t: &str) {
    for c in t.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(s, "\\u{:04x}", c as u32);
            }
            c => s.push(c),
        }
    }
}

/// The JSON of a decode run.
pub fn to_json(ev: &[Event], act: &[Active], now: f64) -> String {
    let mut s = String::from("{\"ev\":[");
    for (i, e) in ev.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(s, "{{\"id\":{},\"hz\":{:.1},\"text\":\"", e.id, e.hz);
        esc(&mut s, &e.text);
        let _ = write!(
            s,
            "\",\"wpm\":{:.1},\"snr\":{:.1},\"t\":{:.3}}}",
            e.wpm, e.snr, e.t
        );
    }
    s.push_str("],\"act\":[");
    for (i, a) in act.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(
            s,
            "{{\"id\":{},\"hz\":{:.1},\"wpm\":{:.1},\"snr\":{:.1}}}",
            a.id, a.hz, a.wpm, a.snr
        );
    }
    let _ = write!(s, "],\"now\":{now:.3}}}");
    s
}

#[unsafe(no_mangle)]
pub extern "C" fn cw_buffer(n: usize) -> *mut f32 {
    STATE.with_borrow_mut(|s| {
        s.input.resize(n, 0.0);
        s.input.as_mut_ptr()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn cw_push(n: usize) -> usize {
    STATE.with_borrow_mut(|s| {
        let n = n.min(s.input.len());
        let State { skimmer, input, .. } = s;
        skimmer.push(&input[..n]);
        output(s)
    })
}

/// The JSON of a decode run (its length), or 0 between runs.
fn output(s: &mut State) -> usize {
    if s.skimmer.runs == s.runs {
        return 0;
    }
    s.runs = s.skimmer.runs;
    let ev = s.skimmer.poll();
    s.output = to_json(&ev, &s.skimmer.active(), s.skimmer.now());
    s.output.len()
}

#[unsafe(no_mangle)]
pub extern "C" fn cw_output() -> *const u8 {
    STATE.with_borrow(|s| s.output.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn cw_band(lo: f64, hi: f64) {
    STATE.with_borrow_mut(|s| {
        if (lo, hi) != s.band {
            s.band = (lo, hi);
            s.skimmer.set_band(lo, hi);
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn cw_neural(on: u32) {
    STATE.with_borrow_mut(|s| {
        s.neural = on != 0;
        s.skimmer.neural = s.neural;
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn cw_reset() {
    STATE.with_borrow_mut(fresh);
}

/// A new skimmer for the current input.
fn fresh(s: &mut State) {
    s.skimmer = match s.wide {
        Some((rate, lo, hi)) => Skimmer::new_iq(rate, lo, hi),
        None => Skimmer::new(s.band.0, s.band.1),
    };
    s.skimmer.neural = s.neural;
    s.runs = 0;
}

#[unsafe(no_mangle)]
pub extern "C" fn cw_wide(rate: f64, lo: f64, hi: f64) {
    STATE.with_borrow_mut(|s| {
        s.wide = (rate > 0.0).then_some((rate, lo, hi));
        fresh(s);
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn cw_buffer_iq(n: usize) -> *mut f32 {
    STATE.with_borrow_mut(|s| {
        s.input_iq.resize(2 * n, 0.0);
        s.input_iq.as_mut_ptr()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn cw_push_iq(n: usize) -> usize {
    STATE.with_borrow_mut(|s| {
        let n = n.min(s.input_iq.len() / 2);
        let iq: Vec<num_complex::Complex32> = s.input_iq[..2 * n]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| num_complex::Complex32::new(c[0], c[1]))
            .collect();
        s.skimmer.push_iq(&iq);
        output(s)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_as_json() {
        let e = Event {
            id: 3,
            hz: 700.04,
            text: "CQ \"X\"".into(),
            wpm: 22.04,
            snr: 7.0,
            t: 1.5,
        };
        let a = Active {
            id: 3,
            hz: 700.0,
            wpm: 22.0,
            snr: 7.0,
        };
        assert_eq!(
            to_json(&[e], &[a], 12.0),
            r#"{"ev":[{"id":3,"hz":700.0,"text":"CQ \"X\"","wpm":22.0,"snr":7.0,"t":1.500}],"act":[{"id":3,"hz":700.0,"wpm":22.0,"snr":7.0}],"now":12.000}"#
        );
    }
}
