// SPDX-License-Identifier: GPL-3.0-or-later
//! The JTTY receiver (web888-jtty) for the web UI: built for
//! wasm32-unknown-unknown (`scripts/build-wasm.sh`) and run in a Web
//! Worker (`ui/src/jtty/worker.ts`). A plain C ABI, one receiver per
//! instance:
//!
//! * `jtty_buffer(n)`: room for n samples (12 kHz mono f32); returns where
//!   the caller writes them
//! * `jtty_push(n)`: decodes them; returns the length of the JSON array of
//!   completed messages at `jtty_output()`
//!   (`[{"text":..,"hz":..,"start":..,"frames":..}]`, `start` in samples
//!   since the last reset)
//! * `jtty_reset()`: a new stream (after retuning)

use std::cell::RefCell;
use web888_jtty::{Message, Receiver};

struct State {
    receiver: Receiver,
    input: Vec<f32>,
    output: String,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        receiver: Receiver::new(),
        input: Vec::new(),
        output: String::new(),
    });
}

/// The JSON array of `messages`.
pub fn to_json(messages: &[Message]) -> String {
    let mut s = String::from("[");
    for (i, m) in messages.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str("{\"text\":\"");
        for c in m.text.chars() {
            match c {
                '"' => s.push_str("\\\""),
                '\\' => s.push_str("\\\\"),
                c if (c as u32) < 0x20 => s.push_str(&format!("\\u{:04x}", c as u32)),
                c => s.push(c),
            }
        }
        s.push_str(&format!(
            "\",\"hz\":{},\"start\":{},\"frames\":{}}}",
            m.frequency_hz, m.start_sample, m.frames
        ));
    }
    s.push(']');
    s
}

#[unsafe(no_mangle)]
pub extern "C" fn jtty_buffer(n: usize) -> *mut f32 {
    STATE.with_borrow_mut(|s| {
        s.input.resize(n, 0.0);
        s.input.as_mut_ptr()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn jtty_push(n: usize) -> usize {
    STATE.with_borrow_mut(|s| {
        let n = n.min(s.input.len());
        let messages = s.receiver.push(&s.input[..n]);
        s.output = to_json(&messages);
        s.output.len()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn jtty_output() -> *const u8 {
    STATE.with_borrow(|s| s.output.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn jtty_reset() {
    STATE.with_borrow_mut(|s| s.receiver = Receiver::new());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_as_json() {
        let m = Message {
            text: "SAY \"HI\" 73".into(),
            frequency_hz: 1500,
            start_sample: -120,
            frames: 3,
        };
        assert_eq!(
            to_json(&[m.clone(), m]),
            r#"[{"text":"SAY \"HI\" 73","hz":1500,"start":-120,"frames":3},{"text":"SAY \"HI\" 73","hz":1500,"start":-120,"frames":3}]"#
        );
        assert_eq!(to_json(&[]), "[]");
    }
}
