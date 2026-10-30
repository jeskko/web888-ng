// SPDX-License-Identifier: GPL-3.0-or-later
//! JTTY, the keyboard mode of WSJT-X 3.3: text in 1 to 16 frames of 4-GFSK
//! (59 symbols at 32 baud, 31.25 Hz tones, about 127 Hz wide), sent at any
//! time (no slots).
//!
//! A port to Rust of jtty-cuf (MIT, Copyright (c) 2026 Jouni OH3CUF,
//! `LICENSE-jtty-cuf`; https://github.com/j0uni/jtty-cuf), an independent
//! implementation of the mode written from its public documentation; the
//! mode itself, its frame format, code and message grammar are the WSJT-X
//! developers' work. The receiver here is a stream ([`Receiver`]); for a
//! finite recording, [`receive_messages`] decodes it as jtty-cuf does.

// NaN-aware tests (`!(a < b)`) and index loops kept as in jtty-cuf, so the
// port reads against its source
#![allow(clippy::neg_cmp_op_on_partial_ord, clippy::needless_range_loop)]

mod assembly;
pub mod codec;
mod fec;
mod gfsk;
pub mod protocol;
mod receiver;

pub use assembly::Work;
pub use codec::{MessageProfile, modulate_message, pack_literal_message};
pub use receiver::{Message, Receiver, SEARCH_HIGH_HZ, SEARCH_LOW_HZ, receive_messages};
