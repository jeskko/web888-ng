// SPDX-License-Identifier: GPL-3.0-or-later
//! The signal identifier (web888-sigid) for the web UI: built for
//! wasm32-unknown-unknown (`scripts/build-wasm.sh`) and run in a Web
//! Worker (`ui/src/sigid/worker.ts`). A plain C ABI, one scanner per
//! instance:
//!
//! * `sig_buffer(n)`: room for n samples (12 kHz mono f32); returns where
//!   the caller writes them
//! * `sig_push(n)`: runs them through; after an identification (one
//!   every 2 s) returns the length of the JSON at `sig_output()`, else 0:
//!   `{"sig":[{"id":..,"hz":..,"lo":..,"hi":..,"db":..,"runs":..,
//!   "top":[["ft8",0.93],..]}]}` (audio Hz; the three likeliest classes)
//! * `sig_band(lo, hi)`: the audio band searched (Hz)
//! * `sig_lsb(on)`: the audio is LSB (its spectrum the RF's upside down)
//! * `sig_reset()`: a new stream (after retuning)

use std::cell::RefCell;
use std::fmt::Write;
use web888_sigid::scan::{Found, Scanner};

struct State {
    scanner: Scanner,
    input: Vec<f32>,
    output: String,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        scanner: Scanner::new(),
        input: Vec::new(),
        output: String::new(),
    });
}

/// The JSON of the signals.
pub fn to_json(found: &[Found]) -> String {
    let mut s = String::from("{\"sig\":[");
    for (i, f) in found.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(
            s,
            "{{\"id\":{},\"hz\":{:.1},\"lo\":{:.1},\"hi\":{:.1},\"db\":{:.1},\"runs\":{},\"top\":[",
            f.id, f.hz, f.lo, f.hi, f.db, f.runs
        );
        for (j, (c, p)) in f.top(3).iter().enumerate() {
            if j > 0 {
                s.push(',');
            }
            let _ = write!(s, "[\"{c}\",{p:.3}]");
        }
        s.push_str("]}");
    }
    s.push_str("]}");
    s
}

#[unsafe(no_mangle)]
pub extern "C" fn sig_buffer(n: usize) -> *mut f32 {
    STATE.with_borrow_mut(|s| {
        s.input.clear();
        s.input.resize(n, 0.0);
        s.input.as_mut_ptr()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn sig_push(n: usize) -> usize {
    STATE.with_borrow_mut(|s| {
        let n = n.min(s.input.len());
        let pcm = std::mem::take(&mut s.input);
        let ran = s.scanner.push(&pcm[..n]);
        s.input = pcm;
        if !ran {
            return 0;
        }
        s.output = to_json(s.scanner.found());
        s.output.len()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn sig_output() -> *const u8 {
    STATE.with_borrow(|s| s.output.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn sig_band(lo: f64, hi: f64) {
    STATE.with_borrow_mut(|s| s.scanner.set_band(lo, hi));
}

#[unsafe(no_mangle)]
pub extern "C" fn sig_lsb(on: u32) {
    STATE.with_borrow_mut(|s| s.scanner.lsb = on != 0);
}

#[unsafe(no_mangle)]
pub extern "C" fn sig_reset() {
    STATE.with_borrow_mut(|s| s.scanner.reset());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_shape() {
        assert_eq!(to_json(&[]), "{\"sig\":[]}");
    }
}
