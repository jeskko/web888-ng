// SPDX-License-Identifier: GPL-3.0-or-later
//! The JS8 decoder (web888-js8) for the web UI: built for
//! wasm32-unknown-unknown (`scripts/build-wasm.sh`) and run in a Web
//! Worker (`ui/src/js8/worker.ts`). A plain C ABI:
//!
//! * `js8_buffer(n)`: room for n samples (12 kHz mono f32, USB, from a
//!   period's start)
//! * `js8_decode(submode, n)`: decodes them as one period of `submode` (0
//!   Normal, 1 Fast, 2 Turbo, 3 Slow); returns the length of the JSON at
//!   `js8_output()`: `{"frames":[{"hz","dt","snr","flags","kind","text",
//!   "from","to","grid"}]}` (`hz` the audio frequency of tone 0; `flags`
//!   1 first frame of a message, 2 last, 4 fast data; `kind`
//!   "heartbeat", "compound", "compound directed", "directed", "data";
//!   `from`, `to`, `grid` null when the frame doesn't say)
//!
//! The host provides `env.web888_now_ms()` (`performance.now()`), which
//! the FT8 decoder underneath uses for timing (web888-clock).

use std::cell::RefCell;
use std::fmt::Write;
use web888_js8::message::{self, Kind};
use web888_js8::{Frame, Frames, Submode};

struct State {
    frames: Frames,
    input: Vec<f32>,
    output: String,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        frames: Frames::new(),
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

fn opt(s: &mut String, key: &str, v: &Option<String>) {
    let _ = write!(s, ",\"{key}\":");
    match v {
        Some(v) => esc(s, v),
        None => s.push_str("null"),
    }
}

/// The JSON of a period's frames.
pub fn to_json(frames: &[Frame]) -> String {
    let mut s = String::from("{\"frames\":[");
    let mut first = true;
    for f in frames {
        let Some(u) = message::unpack(f.bits >> 3, f.kind) else {
            continue;
        };
        if !first {
            s.push(',');
        }
        first = false;
        let kind = match u.kind {
            Kind::Heartbeat => "heartbeat",
            Kind::Compound => "compound",
            Kind::CompoundDirected => "compound directed",
            Kind::Directed => "directed",
            Kind::Data => "data",
        };
        let _ = write!(
            s,
            "{{\"hz\":{:.1},\"dt\":{:.2},\"snr\":{:.0},\"flags\":{},\"kind\":\"{kind}\",\"text\":",
            f.freq, f.dt, f.snr, f.kind
        );
        esc(&mut s, &u.text);
        opt(&mut s, "from", &u.from);
        opt(&mut s, "to", &u.to);
        opt(&mut s, "grid", &u.grid);
        s.push('}');
    }
    s.push_str("]}");
    s
}

#[unsafe(no_mangle)]
pub extern "C" fn js8_buffer(n: usize) -> *mut f32 {
    STATE.with_borrow_mut(|s| {
        s.input.resize(n, 0.0);
        s.input.as_mut_ptr()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn js8_decode(submode: u32, n: usize) -> usize {
    STATE.with_borrow_mut(|s| {
        let sm = Submode::ALL[(submode as usize).min(3)];
        let n = n.min(s.input.len());
        let State { frames, input, .. } = s;
        let f = frames.decode(sm, &input[..n]);
        s.output = to_json(&f);
        s.output.len()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn js8_output() -> *const u8 {
    STATE.with_borrow(|s| s.output.as_ptr())
}

#[cfg(test)]
mod tests {
    use super::*;
    use web888_ftx::js8;

    #[test]
    fn json() {
        // a directed frame: N0ABC to @ALLCALL, " SNR?" (command 0), no number
        let from = ((((36 * 36 + 23) * 10) * 27) * 27 + 1) * 27 + 2u128;
        let to = 37 * 36 * 10 * 27 * 27 * 27 + 2u128;
        let payload = (3u128 << 69) | (from << 41) | (to << 13);
        let f = Frame {
            submode: Submode::Normal,
            freq: 1234.5,
            dt: 0.12,
            snr: -7.0,
            text: js8::text(payload << 3 | 3).0,
            kind: 3,
            bits: payload << 3 | 3,
        };
        assert_eq!(
            to_json(&[f]),
            r#"{"frames":[{"hz":1234.5,"dt":0.12,"snr":-7,"flags":3,"kind":"directed","text":"N0ABC: @ALLCALL SNR? ","from":"N0ABC","to":"@ALLCALL","grid":null}]}"#
        );
    }
}
