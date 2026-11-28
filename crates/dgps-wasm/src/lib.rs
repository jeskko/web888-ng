// SPDX-License-Identifier: GPL-3.0-or-later
//! The DGPS beacon skimmer (web888-dgps) for the web UI: built for
//! wasm32-unknown-unknown (`scripts/build-wasm.sh`) and run in a Web
//! Worker (`ui/src/dgps/worker.ts`). A plain C ABI, one skimmer per
//! instance:
//!
//! * `dgps_buffer(n)`: room for n samples (12 kHz mono f32)
//! * `dgps_push(n)`: runs them through; returns the length of the JSON at
//!   `dgps_output()` when there is something new (a message, or a run
//!   every 2 s), else 0: `{"ev":[...],"act":[...],"now":..}` with events
//!   `{"k":"lock","id","hz","baud"}` and `{"k":"msg","id","hz","t",
//!   "type","sid","z","seq","health","ok","n"}` (ok: words read of n)
//!   plus, where decoded, `"gps"` or `"glo"` (satellite IDs), `"pos"`
//!   ([lat, lon, h]), `"alm"` ([[lat, lon, range km, kHz, health, id,
//!   baud], ...]) or `"text"`; and beacons `{"id","hz","baud","q","w",
//!   "sync"}` (audio Hz; times in s since the reset)
//! * `dgps_band(lo, hi)`: the audio band searched (Hz)
//! * `dgps_reset()`: a new stream (after retuning)

use std::cell::RefCell;
use std::fmt::Write;
use web888_dgps::{Active, Body, Event, Skimmer};

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
            c if (c as u32) < 0x20 => {
                let _ = write!(s, "\\u{:04x}", c as u32);
            }
            c => s.push(c),
        }
    }
    s.push('"');
}

fn ids(s: &mut String, v: impl Iterator<Item = u32>) {
    s.push('[');
    for (i, x) in v.enumerate() {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(s, "{x}");
    }
    s.push(']');
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
            Event::Msg { id, hz, msg } => {
                let ok = msg.words.iter().filter(|w| w.is_some()).count();
                let _ = write!(
                    s,
                    "{{\"k\":\"msg\",\"id\":{id},\"hz\":{hz:.1},\"t\":{:.3},\"type\":{},\"sid\":{},\"z\":{:.1},\"seq\":{},\"health\":{},\"ok\":{ok},\"n\":{}",
                    msg.t,
                    msg.kind,
                    msg.station,
                    msg.z,
                    msg.seq,
                    msg.health,
                    msg.words.len()
                );
                match &msg.body {
                    Body::None => {}
                    Body::Corr { glonass, sats } => {
                        s.push_str(if *glonass { ",\"glo\":" } else { ",\"gps\":" });
                        ids(&mut s, sats.iter().map(|x| x.prn));
                    }
                    Body::Pos { lat, lon, h } => {
                        let _ = write!(s, ",\"pos\":[{lat:.5},{lon:.5},{h:.1}]");
                    }
                    Body::Almanac(b) => {
                        s.push_str(",\"alm\":[");
                        for (j, x) in b.iter().enumerate() {
                            if j > 0 {
                                s.push(',');
                            }
                            let _ = write!(
                                s,
                                "[{:.4},{:.4},{},{:.1},{},{},{}]",
                                x.lat, x.lon, x.range, x.khz, x.health, x.id, x.baud
                            );
                        }
                        s.push(']');
                    }
                    Body::Text(t) => {
                        s.push_str(",\"text\":");
                        esc(&mut s, t);
                    }
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
            "{{\"id\":{},\"hz\":{:.1},\"baud\":{},\"q\":{:.2},\"w\":{:.2},\"sync\":{}}}",
            a.id, a.hz, a.baud, a.quality, a.words, a.synced
        );
    }
    let _ = write!(s, "],\"now\":{now:.3}}}");
    s
}

#[unsafe(no_mangle)]
pub extern "C" fn dgps_buffer(n: usize) -> *mut f32 {
    STATE.with_borrow_mut(|s| {
        s.input.resize(n, 0.0);
        s.input.as_mut_ptr()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn dgps_push(n: usize) -> usize {
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
pub extern "C" fn dgps_output() -> *const u8 {
    STATE.with_borrow(|s| s.output.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn dgps_band(lo: f64, hi: f64) {
    STATE.with_borrow_mut(|s| {
        if (lo, hi) != s.band {
            s.band = (lo, hi);
            s.skimmer.set_band(lo, hi);
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn dgps_reset() {
    STATE.with_borrow_mut(|s| {
        s.skimmer = Skimmer::new(s.band.0, s.band.1);
        s.runs = 0;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use web888_dgps::Msg;

    #[test]
    fn json() {
        let msg = |kind, body| Msg {
            t: 1.5,
            kind,
            station: 606,
            z: 2086.2,
            seq: 1,
            health: 0,
            words: vec![Some(0), None],
            body,
        };
        let ev = vec![
            Event::Lock {
                id: 1,
                hz: 1000.0,
                baud: 100.0,
            },
            Event::Msg {
                id: 1,
                hz: 1000.0,
                msg: msg(16, Body::Text("A \"B\"".into())),
            },
            Event::Msg {
                id: 1,
                hz: 1000.0,
                msg: msg(
                    3,
                    Body::Pos {
                        lat: 60.5,
                        lon: 21.25,
                        h: 30.0,
                    },
                ),
            },
        ];
        let act = vec![Active {
            id: 1,
            hz: 1000.25,
            baud: 100.0,
            quality: 0.951,
            words: 0.5,
            synced: true,
        }];
        assert_eq!(
            to_json(&ev, &act, 4.0),
            r#"{"ev":[{"k":"lock","id":1,"hz":1000.0,"baud":100},{"k":"msg","id":1,"hz":1000.0,"t":1.500,"type":16,"sid":606,"z":2086.2,"seq":1,"health":0,"ok":1,"n":2,"text":"A \"B\""},{"k":"msg","id":1,"hz":1000.0,"t":1.500,"type":3,"sid":606,"z":2086.2,"seq":1,"health":0,"ok":1,"n":2,"pos":[60.50000,21.25000,30.0]}],"act":[{"id":1,"hz":1000.2,"baud":100,"q":0.95,"w":0.50,"sync":true}],"now":4.000}"#
        );
    }
}
