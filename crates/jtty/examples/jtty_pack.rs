// SPDX-License-Identifier: GPL-3.0-or-later
//! Prints how text packs into frames:
//!   cargo run -p web888-jtty --example jtty_pack -- "CQ K1ABC FN42" ...
use web888_jtty::{MessageProfile, pack_literal_message};

fn main() {
    for text in std::env::args().skip(1) {
        match pack_literal_message(&text, MessageProfile::Unknown) {
            Ok(m) => {
                print!("{text}:");
                for f in &m.frames {
                    print!(" [{} {} {:08x}]", f.atom, f.rendering, f.wire.message_word);
                }
                println!();
            }
            Err(e) => println!("{text}: error {e}"),
        }
    }
}
