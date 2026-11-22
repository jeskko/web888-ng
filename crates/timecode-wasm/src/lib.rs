// SPDX-License-Identifier: GPL-3.0-or-later
//! The time-code receiver (web888-timecode) for the web UI: built for
//! wasm32-unknown-unknown (`scripts/build-wasm.sh`) and run in a Web
//! Worker (`ui/src/timecode/worker.ts`). A plain C ABI, one receiver per
//! instance:
//!
//! * `tc_start(station, tone)`: a new receiver (station: the index in
//!   `Station::ALL`, 0 DCF77 .. 12 RWM) for a tone at this audio
//!   frequency (Hz; WWV's 100 Hz and BPM's 1 kHz in AM audio); stream
//!   time restarts at 0
//! * `tc_buffer(n)`: room for n samples (12 kHz mono f32)
//! * `tc_push(n)`: runs them through; returns the length of the JSON at
//!   `tc_output()` when something is new, else 0:
//!   `{"sec":[{"t","s","q","i"}],"min":[{"t","utc","ok","confirmed","named","tx","local","notes","sym"}],"level","now"}`
//!   (t: stream s; s: the symbol; q: its quality; i: its second in the
//!   minute, or -1; utc: Unix s of the minute's start; named: false for
//!   stations without a time code (BPM, RWM: utc means nothing, the
//!   listener's clock names the minute); tx: which transmitter sent it
//!   (WWV or WWVH) or null)

use std::cell::RefCell;
use std::fmt::Write;
use web888_timecode::{Event, Receiver, Station};

const FS: f64 = 12_000.0;

struct State {
    rx: Option<Receiver>,
    n: u64,
    input: Vec<f32>,
    output: String,
}

thread_local! {
    static STATE: RefCell<State> = const { RefCell::new(State { rx: None, n: 0, input: Vec::new(), output: String::new() }) };
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

/// The JSON of events (and the level and stream time).
pub fn to_json(ev: &[Event], level: f32, now: f64) -> String {
    let mut s = String::from("{\"sec\":[");
    let mut first = true;
    for e in ev {
        if let Event::Second(x) = e {
            if !first {
                s.push(',');
            }
            first = false;
            let _ = write!(
                s,
                "{{\"t\":{:.3},\"s\":\"{}\",\"q\":{:.2},\"i\":{}}}",
                x.t,
                x.sym,
                x.quality,
                x.index.map(i32::from).unwrap_or(-1)
            );
        }
    }
    s.push_str("],\"min\":[");
    first = true;
    for e in ev {
        if let Event::Minute(m) = e {
            if !first {
                s.push(',');
            }
            first = false;
            let _ = write!(
                s,
                "{{\"t\":{:.3},\"utc\":{},\"ok\":{},\"confirmed\":{},\"named\":{},\"tx\":",
                m.t, m.utc, m.ok, m.confirmed, m.named
            );
            match m.transmitter {
                Some(x) => esc(&mut s, x),
                None => s.push_str("null"),
            }
            s.push_str(",\"local\":");
            esc(&mut s, &m.local);
            s.push_str(",\"notes\":[");
            for (i, n) in m.notes.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                esc(&mut s, n);
            }
            s.push_str("],\"sym\":");
            esc(&mut s, &m.symbols);
            s.push('}');
        }
    }
    let _ = write!(s, "],\"level\":{level:.1},\"now\":{now:.3}}}");
    s
}

#[unsafe(no_mangle)]
pub extern "C" fn tc_start(station: u32, tone: f64) {
    STATE.with_borrow_mut(|s| {
        s.rx = Station::ALL
            .get(station as usize)
            .map(|&st| Receiver::new(st, FS, tone, 0.0));
        s.n = 0;
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn tc_buffer(n: usize) -> *mut f32 {
    STATE.with_borrow_mut(|s| {
        s.input.resize(n, 0.0);
        s.input.as_mut_ptr()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn tc_push(n: usize) -> usize {
    STATE.with_borrow_mut(|s| {
        let n = n.min(s.input.len());
        let State {
            rx,
            input,
            n: total,
            output,
        } = s;
        let Some(rx) = rx else { return 0 };
        rx.push(&input[..n]);
        *total += n as u64;
        let ev = rx.poll();
        if ev.is_empty() {
            return 0;
        }
        *output = to_json(&ev, rx.level, *total as f64 / FS);
        output.len()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn tc_output() -> *const u8 {
    STATE.with_borrow(|s| s.output.as_ptr())
}
