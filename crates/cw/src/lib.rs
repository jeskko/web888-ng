// SPDX-License-Identifier: GPL-3.0-or-later
//! CW (Morse) decoding for the web UI: a decoder for one keyed carrier
//! ([`decode`]) and a skimmer that finds and decodes every keyed carrier
//! in a passband of audio ([`Skimmer`]).
//!
//! A port to Rust of datarx's `wahrx/cw.py` and `wahrx/cwskim.py` (MIT,
//! by this project's author), on real 12 kHz audio rather than wideband
//! IQ. Its synthetic tests and their bars (character error rates at given
//! SNR, jitter and fading) come along.

pub mod channel;
pub mod decode;
pub mod dsp;
pub mod morse;
pub mod skim;
pub mod synth;

pub use channel::AUDIO_RATE;
pub use decode::{CwResult, DecodeOpts, ENV_FS, decode};
pub use skim::{Active, Event, Skimmer};
