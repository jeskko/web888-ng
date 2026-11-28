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
//!   plus, where decoded, `"gps"` or `"glo"` ([[ID, correction m or
//!   null (don't use), its rate m/s, UDRE 0-3, IOD], ...]), `"pos"`
//!   ([lat, lon, h]), `"alm"` ([[lat, lon, range km, kHz, health, id,
//!   baud], ...]) or `"text"`; and beacons `{"id","hz","baud","q","w",
//!   "sync"}` (audio Hz; times in s since the reset)
//! * `dgps_band(lo, hi)`: the audio band searched (Hz)
//! * `dgps_wide(rate, lo, hi)`: complex IQ at `rate` (a channelizer
//!   bin), beacons between `lo` and `hi` Hz from its centre; `rate` 0:
//!   back to audio. `dgps_buffer_iq(n)` / `dgps_push_iq(n)` take n
//!   complex samples (I, Q interleaved f32) as `dgps_buffer` /
//!   `dgps_push` take audio
//! * `dgps_reset()`: a new stream (after retuning)

use std::cell::RefCell;
use std::fmt::Write;
use web888_dgps::{Active, Body, Event, Skimmer};

struct State {
    skimmer: Skimmer,
    /// IQ input: (rate, lo, hi).
    wide: Option<(f64, f64, f64)>,
    input_iq: Vec<f32>,
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
                        s.push_str(if *glonass { ",\"glo\":[" } else { ",\"gps\":[" });
                        for (j, x) in sats.iter().enumerate() {
                            if j > 0 {
                                s.push(',');
                            }
                            let prc = x.prc.map_or("null".to_string(), |v| format!("{v:.2}"));
                            let _ =
                                write!(s, "[{},{prc},{:.3},{},{}]", x.prn, x.rrc, x.udre, x.iod);
                        }
                        s.push(']');
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

/// The JSON's length when there is something new, else 0.
fn output(s: &mut State) -> usize {
    let ev = s.skimmer.poll();
    if ev.is_empty() && s.skimmer.runs == s.runs {
        return 0;
    }
    s.runs = s.skimmer.runs;
    s.output = to_json(&ev, &s.skimmer.active(), s.skimmer.now());
    s.output.len()
}

/// A new skimmer for the input (audio band, or the IQ bin).
fn fresh(s: &mut State) {
    s.skimmer = match s.wide {
        Some((rate, lo, hi)) => Skimmer::new_iq(rate, lo, hi),
        None => Skimmer::new(s.band.0, s.band.1),
    };
    s.runs = 0;
}

#[unsafe(no_mangle)]
pub extern "C" fn dgps_push(n: usize) -> usize {
    STATE.with_borrow_mut(|s| {
        let n = n.min(s.input.len());
        let State { skimmer, input, .. } = s;
        skimmer.push(&input[..n]);
        output(s)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn dgps_wide(rate: f64, lo: f64, hi: f64) {
    STATE.with_borrow_mut(|s| {
        s.wide = (rate > 0.0).then_some((rate, lo, hi));
        fresh(s);
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn dgps_buffer_iq(n: usize) -> *mut f32 {
    STATE.with_borrow_mut(|s| {
        s.input_iq.resize(2 * n, 0.0);
        s.input_iq.as_mut_ptr()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn dgps_push_iq(n: usize) -> usize {
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

#[unsafe(no_mangle)]
pub extern "C" fn dgps_output() -> *const u8 {
    STATE.with_borrow(|s| s.output.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn dgps_band(lo: f64, hi: f64) {
    STATE.with_borrow_mut(|s| {
        if (lo, hi) != s.band {
            s.band = (lo, hi);
            if s.wide.is_none() {
                s.skimmer.set_band(lo, hi);
            }
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn dgps_reset() {
    STATE.with_borrow_mut(fresh);
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

    #[test]
    fn corrections() {
        use web888_dgps::rtcm::Sat;
        let ev = vec![Event::Msg {
            id: 2,
            hz: 900.0,
            msg: Msg {
                t: 0.0,
                kind: 9,
                station: 1,
                z: 0.0,
                seq: 0,
                health: 0,
                words: vec![Some(0)],
                body: Body::Corr {
                    glonass: false,
                    sats: vec![
                        Sat {
                            prn: 12,
                            iod: 40,
                            udre: 0,
                            prc: Some(-3.46),
                            rrc: 0.004,
                        },
                        Sat {
                            prn: 3,
                            iod: 7,
                            udre: 3,
                            prc: None,
                            rrc: 0.0,
                        },
                    ],
                },
            },
        }];
        assert_eq!(
            to_json(&ev, &[], 1.0),
            r#"{"ev":[{"k":"msg","id":2,"hz":900.0,"t":0.000,"type":9,"sid":1,"z":0.0,"seq":0,"health":0,"ok":1,"n":1,"gps":[[12,-3.46,0.004,0,40],[3,null,0.000,3,7]]}],"act":[],"now":1.000}"#
        );
    }
}
