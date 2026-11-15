// SPDX-License-Identifier: GPL-3.0-or-later
//! The HFDL decoder (web888-hfdl) for the web UI: built for
//! wasm32-unknown-unknown (`scripts/build-wasm.sh`) and run in a Web
//! Worker (`ui/src/hfdl/worker.ts`). A plain C ABI, one channel per
//! instance:
//!
//! * `hfdl_buffer(n)`: room for n samples (12 kHz mono f32, USB)
//! * `hfdl_push(n)`: runs them through; returns the length of the JSON at
//!   `hfdl_output()` when there is something new (a burst, or every 2 s),
//!   else 0: `{"ev":[...],"bursts","decoded","now"}`, each event a burst
//!   `{"t","bps","slot":"S"|"D","f","snr","dir","gs","items":[...]}`
//!   (`dir` "up", "down", "squitter", or "" when no CRC passed; `gs` the
//!   ground station's ID or null), each item `{"ac","k","text","flight",
//!   "reg","icao","lat","lon"}` (`ac`, `icao`, `lat`/`lon` when known;
//!   `icao` hex). Times in s since the reset.
//! * `hfdl_centre(hz)`: the bursts' centre in the audio (the listed
//!   frequency + 1440 Hz, less the dial); a new stream
//! * `hfdl_reset()`: a new stream (after retuning)
//! * `hfdl_stations()`: the ground station table as JSON at
//!   `hfdl_output()` (its length): `[{"id","name","lat","lon","f":[kHz]}]`

use std::cell::RefCell;
use std::fmt::Write;
use web888_hfdl::skim::{Decoder, Message};
use web888_hfdl::stations::STATIONS;

const RATE: f64 = 12000.0;

struct State {
    dec: Decoder,
    centre: f64,
    last: f64,
    input: Vec<f32>,
    output: String,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        dec: Decoder::new(RATE, 1440.0),
        centre: 1440.0,
        last: 0.0,
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
pub fn to_json(ev: &[Message], bursts: u64, decoded: u64, now: f64) -> String {
    let mut s = String::from("{\"ev\":[");
    for (i, m) in ev.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(
            s,
            "{{\"t\":{:.2},\"bps\":{},\"slot\":\"{}\",\"f\":{:.1},\"snr\":{:.1},\"dir\":\"{}\",\"gs\":{},\"items\":[",
            m.t,
            m.bps,
            if m.double { 'D' } else { 'S' },
            m.freq,
            m.snr.max(-99.0),
            m.dir,
            m.gs.map_or("null".to_string(), |g| g.to_string())
        );
        for (j, it) in m.items.iter().enumerate() {
            if j > 0 {
                s.push(',');
            }
            let _ = write!(
                s,
                "{{\"ac\":{},\"k\":\"{}\",\"text\":",
                it.ac.map_or("null".to_string(), |a| a.to_string()),
                it.kind
            );
            esc(&mut s, &it.text);
            s.push_str(",\"flight\":");
            esc(&mut s, &it.flight);
            s.push_str(",\"reg\":");
            esc(&mut s, &it.reg);
            match it.icao {
                Some(i) => {
                    let _ = write!(s, ",\"icao\":\"{i:06X}\"");
                }
                None => s.push_str(",\"icao\":null"),
            }
            match it.pos {
                Some((a, o)) => {
                    let _ = write!(s, ",\"lat\":{a:.4},\"lon\":{o:.4}");
                }
                None => s.push_str(",\"lat\":null,\"lon\":null"),
            }
            s.push('}');
        }
        s.push_str("]}");
    }
    let _ = write!(
        s,
        "],\"bursts\":{bursts},\"decoded\":{decoded},\"now\":{now:.2}}}"
    );
    s
}

pub fn stations_json() -> String {
    let mut s = String::from("[");
    for (i, st) in STATIONS.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(s, "{{\"id\":{},\"name\":", st.id);
        esc(&mut s, st.name);
        let f: Vec<String> = st.freqs.iter().map(|f| format!("{f:.0}")).collect();
        let _ = write!(
            s,
            ",\"lat\":{},\"lon\":{},\"f\":[{}]}}",
            st.lat,
            st.lon,
            f.join(",")
        );
    }
    s.push(']');
    s
}

#[unsafe(no_mangle)]
pub extern "C" fn hfdl_buffer(n: usize) -> *mut f32 {
    STATE.with_borrow_mut(|s| {
        s.input.resize(n, 0.0);
        s.input.as_mut_ptr()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn hfdl_push(n: usize) -> usize {
    STATE.with_borrow_mut(|s| {
        let n = n.min(s.input.len());
        let State { dec, input, .. } = s;
        let ev = dec.push_audio(&input[..n]);
        let now = s.dec.now();
        if ev.is_empty() && now - s.last < 2.0 {
            return 0;
        }
        s.last = now;
        s.output = to_json(&ev, s.dec.bursts, s.dec.decoded, now);
        s.output.len()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn hfdl_output() -> *const u8 {
    STATE.with_borrow(|s| s.output.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn hfdl_centre(hz: f64) {
    STATE.with_borrow_mut(|s| {
        s.centre = hz;
        fresh(s);
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn hfdl_reset() {
    STATE.with_borrow_mut(fresh);
}

fn fresh(s: &mut State) {
    s.dec = Decoder::new(RATE, s.centre);
    s.last = 0.0;
}

#[unsafe(no_mangle)]
pub extern "C" fn hfdl_stations() -> usize {
    STATE.with_borrow_mut(|s| {
        s.output = stations_json();
        s.output.len()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use web888_hfdl::skim::Item;

    #[test]
    fn json() {
        let m = Message {
            t: 1.5,
            bps: 300,
            double: false,
            freq: 5.21,
            snr: 4.6,
            dir: "down",
            gs: Some(7),
            items: vec![Item {
                ac: Some(140),
                kind: "acars",
                text: "label SA: \"hi\"\nthere".into(),
                flight: "XX0001".into(),
                reg: "N0CALL".into(),
                icao: Some(0xABCDEF),
                pos: Some((51.25, -6.5)),
            }],
        };
        let j = to_json(&[m], 3, 1, 9.0);
        assert_eq!(
            j,
            r#"{"ev":[{"t":1.50,"bps":300,"slot":"S","f":5.2,"snr":4.6,"dir":"down","gs":7,"items":[{"ac":140,"k":"acars","text":"label SA: \"hi\"\nthere","flight":"XX0001","reg":"N0CALL","icao":"ABCDEF","lat":51.2500,"lon":-6.5000}]}],"bursts":3,"decoded":1,"now":9.00}"#
        );
        assert!(stations_json().starts_with(r#"[{"id":1,"name":"San Francisco, California","#));
    }
}
