// SPDX-License-Identifier: GPL-3.0-or-later
//! The skimmers' decoders (FT8, FT4: web888-ftx; WSPR: web888-wspr) for
//! the web UI: built for wasm32-unknown-unknown (`scripts/build-wasm.sh`)
//! and run in a Web Worker (`ui/src/digi/worker.ts`) on the audio of the
//! user's own receiver. A plain C ABI, one set of decoders per instance;
//! callsign hashes are kept from slot to slot, as in the skimmers.
//!
//! * `digi_buffer(n)`: room for n samples (12 kHz mono f32, from the slot
//!   boundary); returns where the caller writes them
//! * `digi_decode(mode, deep, n, budget_ms)`: decodes them as one slot
//!   of `mode` (0 FT8, 1 FT4, 2 WSPR) with the skimmers' settings, or
//!   (`deep` 1) a deeper search ([`deep`]), stopping after `budget_ms`
//!   (0: no limit); returns the length of the JSON at `digi_output()`:
//!   `{"ms":..,"cut":..,"decodes":[{"snr":..,"dt":..,"hz":..,"text":..,"drift":..,"uncertain":true}]}`
//!   (`hz` above the dial; `dt` from the mode's nominal start; `drift`
//!   for WSPR only, Hz)
//! * `digi_reset()`: forget the callsigns heard (another band)
//!
//! The host provides `env.web888_now_ms()` (`performance.now()`) for the
//! deadlines (web888-clock).

use std::cell::RefCell;
use std::fmt::Write;
use std::time::Duration;
use web888_clock::Instant;
use web888_ftx::{ft4, ft8, msg::CallHashes};

pub const FT8: u32 = 0;
pub const FT4: u32 = 1;
pub const WSPR: u32 = 2;

/// A decode as reported.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub snr: f32,
    pub dt: f32,
    pub hz: f64,
    pub text: String,
    pub drift: Option<f32>,
    /// Possibly a false decode (`web888_ftx::ft8::uncertain`).
    pub uncertain: bool,
}

/// The deeper search a browser can afford (doc/m4-skimmers.md §3.3,
/// 2026-11-12): more passes, weaker candidates and more of them, no sync
/// symbol gates; WSPR with wsprd's own Fano limit. On the corpora FT8
/// finds 1.2-1.3 % more of jt9's decodes for 2.3 times the time, FT4 a
/// little more for 4.5 times (a few % of a core for one band).
pub fn deep() -> (ft8::Config, ft4::Config, web888_wspr::Config) {
    let f8 = ft8::Config {
        passes: 4,
        sync_min: 1.2,
        max_candidates: 1000,
        sync_symbols_search: 0,
        sync_symbols_coarse: 0,
        sync_symbols: 7,
        ..Default::default()
    };
    let f4 = ft4::Config {
        passes: 4,
        sync_min: 1.2,
        max_candidates: 1000,
        sync_symbols_search: 0,
        sync_symbols_coarse: 0,
        sync_symbols: 6,
        ..Default::default()
    };
    let w = web888_wspr::Config {
        maxcycles: 10_000,
        ..Default::default()
    };
    (f8, f4, w)
}

/// Each mode's decoders, with the skimmers' settings and the deep ones
/// (made when first used).
#[derive(Default)]
pub struct Decoders {
    ft8: [Option<ft8::Decoder>; 2],
    ft4: [Option<ft4::Decoder>; 2],
    wspr: [Option<web888_wspr::Decoder>; 2],
    hashes: CallHashes,
}

impl Decoders {
    /// Decode one slot of `mode` (`deep`: [`deep`]'s settings); `None`
    /// for an unknown mode. Returns the rows and whether the deadline cut
    /// it short.
    pub fn decode(
        &mut self,
        mode: u32,
        deep: bool,
        audio: &[f32],
        deadline: Option<Instant>,
    ) -> Option<(Vec<Row>, bool)> {
        let ftx = |ds: Vec<ft8::Decode>| -> Vec<Row> {
            ds.into_iter()
                .map(|d| Row {
                    uncertain: ft8::uncertain(&d),
                    snr: d.snr,
                    dt: d.dt,
                    hz: d.freq as f64,
                    text: d.message.text,
                    drift: None,
                })
                .collect()
        };
        let k = deep as usize;
        let (c8, c4, cw) = if deep {
            self::deep()
        } else {
            Default::default()
        };
        Some(match mode {
            FT8 => {
                let d = self.ft8[k].get_or_insert_with(|| ft8::Decoder::new(c8));
                let cut = d.stats.cut_short;
                let rows = ftx(d.decode_until(audio, &mut self.hashes, deadline));
                (rows, d.stats.cut_short != cut)
            }
            FT4 => {
                let d = self.ft4[k].get_or_insert_with(|| ft4::Decoder::new(c4));
                let cut = d.stats.cut_short;
                let rows = ftx(d.decode_until(audio, &mut self.hashes, deadline));
                (rows, d.stats.cut_short != cut)
            }
            WSPR => {
                let d = self.wspr[k].get_or_insert_with(|| web888_wspr::Decoder::new(cw));
                let cut = d.cut_short;
                let rows = d
                    .decode_until(audio, deadline)
                    .into_iter()
                    .map(|d| Row {
                        snr: d.snr,
                        dt: d.dt,
                        hz: d.freq,
                        text: d.message.text,
                        drift: Some(d.drift),
                        uncertain: false,
                    })
                    .collect();
                (rows, d.cut_short != cut)
            }
            _ => return None,
        })
    }

    pub fn reset(&mut self) {
        self.hashes = CallHashes::default();
    }
}

fn escape(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
}

/// The JSON of a slot's result.
pub fn to_json(ms: f64, cut: bool, rows: &[Row]) -> String {
    let mut s = format!("{{\"ms\":{ms:.1},\"cut\":{cut},\"decodes\":[");
    for (i, r) in rows.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(
            s,
            "{{\"snr\":{:.0},\"dt\":{:.2},\"hz\":{:.1},\"text\":\"",
            r.snr, r.dt, r.hz
        );
        escape(&mut s, &r.text);
        s.push('"');
        if let Some(d) = r.drift {
            let _ = write!(s, ",\"drift\":{d:.0}");
        }
        if r.uncertain {
            s.push_str(",\"uncertain\":true");
        }
        s.push('}');
    }
    s.push_str("]}");
    s
}

struct State {
    decoders: Decoders,
    input: Vec<f32>,
    output: String,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        decoders: Decoders::default(),
        input: Vec::new(),
        output: String::new(),
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn digi_buffer(n: usize) -> *mut f32 {
    STATE.with_borrow_mut(|s| {
        s.input.resize(n, 0.0);
        s.input.as_mut_ptr()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn digi_decode(mode: u32, deep: u32, n: usize, budget_ms: f64) -> usize {
    STATE.with_borrow_mut(|s| {
        let n = n.min(s.input.len());
        let t = Instant::now();
        let deadline = (budget_ms > 0.0).then(|| t + Duration::from_secs_f64(budget_ms / 1e3));
        let (rows, cut) = s
            .decoders
            .decode(mode, deep != 0, &s.input[..n], deadline)
            .unwrap_or_default();
        s.output = to_json(t.elapsed().as_secs_f64() * 1e3, cut, &rows);
        s.output.len()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn digi_output() -> *const u8 {
    STATE.with_borrow(|s| s.output.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn digi_reset() {
    STATE.with_borrow_mut(|s| s.decoders.reset());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_as_json() {
        let rows = [
            Row {
                snr: -12.4,
                dt: 0.214,
                hz: 1234.56,
                text: "CQ \"N0CALL\" FN42".into(),
                drift: None,
                uncertain: true,
            },
            Row {
                snr: -20.0,
                dt: 1.0,
                hz: 1500.25,
                text: "N0CALL FN42 37".into(),
                drift: Some(-1.0),
                uncertain: false,
            },
        ];
        assert_eq!(
            to_json(12.34, false, &rows),
            r#"{"ms":12.3,"cut":false,"decodes":[{"snr":-12,"dt":0.21,"hz":1234.6,"text":"CQ \"N0CALL\" FN42","uncertain":true},{"snr":-20,"dt":1.00,"hz":1500.2,"text":"N0CALL FN42 37","drift":-1}]}"#
        );
        assert_eq!(
            to_json(0.0, true, &[]),
            r#"{"ms":0.0,"cut":true,"decodes":[]}"#
        );
    }

    #[test]
    fn unknown_mode_decodes_nothing() {
        assert!(
            Decoders::default()
                .decode(9, false, &[0.0; 100], None)
                .is_none()
        );
    }
}
