// SPDX-License-Identifier: GPL-3.0-or-later
//! FSK / RTTY decoding for the web UI, every parameter worked out from the
//! signal: the tones (so shift and centre), the symbol rate, async framing
//! (data bits, parity, stop bits), polarity and character set (ITA2 /
//! US-TTY with unshift-on-space decided from the text, ASCII 7/8 bit LSB
//! or MSB first); SITOR-B / NAVTEX decoded, reversals and other 4-of-7
//! codes identified. [`Skimmer`] finds and decodes every FSK signal in a
//! passband of audio.
//!
//! A port to Rust of datarx's `wahrx/fsk.py` and `wahrx/fskskim.py` (MIT,
//! by this project's author), on real 12 kHz audio rather than wideband
//! IQ. Its synthetic tests come along.

// index loops kept as in fsk.py, so the port reads against its source
#![allow(clippy::needless_range_loop)]

pub mod analyze;
pub mod charset;
pub mod demod;
pub mod skim;
pub mod synth;

pub use analyze::{FskDecoder, FskParams, FskResult, Mode, Parity, analyze, is_valid};
pub use skim::{Active, Event, Skimmer};
