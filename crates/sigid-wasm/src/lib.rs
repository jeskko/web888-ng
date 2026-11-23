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
//! * `sig_held()`: seconds of audio held (an analysis needs 4.096)
//! * `sig_analyse(lo, hi)`: what is in audio lo..hi Hz ("what is this?"):
//!   the length of the JSON at `sig_output()`, `{"an":null}` if nothing
//!   stands over the floor there, else `{"an":{..}}` ([`analysis_json`])

use std::cell::RefCell;
use std::fmt::Write;
use web888_sigid::measure::Line;
use web888_sigid::scan::{Analysis, Found, Scanner};

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

fn num(v: f64) -> String {
    if v.is_finite() {
        format!("{v:.4}")
    } else {
        "null".into()
    }
}

fn line(l: Option<Line>) -> String {
    l.map_or("null".into(), |l| format!("[{},{}]", num(l.at), num(l.db)))
}

/// The JSON of an analysis: audio Hz; `tones` [[hz, dB over the floor]]
/// strongest first; `baudEnv`/`baudFreq` [Bd, dB the line stands out];
/// `psk` [order, carrier offset Hz, line share]; `period`/`envPeriod`
/// [s, correlation]; `top` the five likeliest classes.
pub fn analysis_json(a: Option<&Analysis>) -> String {
    let Some(a) = a else {
        return "{\"an\":null}".into();
    };
    let m = &a.m;
    let tones: Vec<String> = m
        .tones
        .iter()
        .take(16)
        .map(|t| format!("[{},{}]", num(t.at), num(t.db)))
        .collect();
    let top: Vec<String> = a
        .top
        .iter()
        .take(5)
        .map(|(c, p)| format!("[\"{c}\",{p:.4}]"))
        .collect();
    let opt = |v: Option<f64>| v.map_or("null".into(), num);
    format!(
        "{{\"an\":{{\"lo\":{},\"hi\":{},\"centre\":{},\"bw90\":{},\"snr\":{},\"fills\":{},\"duty\":{},\"bursts\":{},\"onMean\":{},\"onShort\":{},\"envCv\":{},\"syllabic\":{},\"tones\":[{}],\"spacing\":{},\"line\":{},\"lineAt\":{},\"sideLo\":{},\"sideHi\":{},\"baudEnv\":{},\"baudFreq\":{},\"psk\":{},\"period\":{},\"envPeriod\":{},\"drift\":{},\"top\":[{}]}}}}",
        num(m.lo),
        num(m.hi),
        num(m.centre),
        num(m.bw90),
        num(m.snr),
        m.fills,
        num(m.duty),
        m.bursts,
        num(m.on_mean),
        num(m.on_short),
        num(m.env_cv),
        num(m.syllabic),
        tones.join(","),
        opt(m.spacing),
        num(m.line),
        num(m.line_at),
        num(m.side_lo),
        num(m.side_hi),
        line(m.baud_env),
        line(m.baud_freq),
        m.psk.map_or("null".into(), |(k, l)| format!(
            "[{k},{},{}]",
            num(l.at),
            num(l.db)
        )),
        line(m.period),
        line(m.env_period),
        opt(m.drift),
        top.join(","),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn sig_held() -> f64 {
    STATE.with_borrow(|s| s.scanner.held())
}

#[unsafe(no_mangle)]
pub extern "C" fn sig_analyse(lo: f64, hi: f64) -> usize {
    STATE.with_borrow_mut(|s| {
        s.output = analysis_json(s.scanner.analyse(lo, hi).as_ref());
        s.output.len()
    })
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
        assert_eq!(analysis_json(None), "{\"an\":null}");
    }

    /// An analysis of a tone is valid JSON with every field.
    #[test]
    fn analysis_shape() {
        let x: Vec<f32> = (0..web888_sigid::measure::NA)
            .map(|i| {
                (0.3 * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 12000.0).sin()) as f32
                    + 1e-3 * ((i * 7919 % 1000) as f32 / 1000.0 - 0.5)
            })
            .collect();
        let a = web888_sigid::scan::analyse(&x, (800.0, 1200.0), (200.0, 2800.0), false)
            .expect("a tone");
        let j = analysis_json(Some(&a));
        for k in [
            "lo", "hi", "centre", "snr", "fills", "tones", "psk", "period", "drift", "top",
        ] {
            assert!(j.contains(&format!("\"{k}\":")), "{k} in {j}");
        }
        assert!(!j.contains("NaN") && !j.contains("inf"), "{j}");
    }
}
