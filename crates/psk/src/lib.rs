// SPDX-License-Identifier: GPL-3.0-or-later
//! PSK31, PSK63 and PSK125 (BPSK, G3PLX's Varicode) for the web UI:
//! [`Skimmer`] finds every BPSK signal in a passband of audio (or complex
//! IQ), works out its carrier and rate, and decodes it.
//!
//! * [`varicode`]: the character code (ASCII 0-127; characters end at two
//!   zero bits).
//! * [`demod`]: one signal at 16 samples a symbol: the matched filter (a
//!   Hann window two symbols long, PSK31's pulse), symbol timing from the
//!   filter's energy, differential detection (no phase change is a 1, a
//!   reversal a 0), a frequency loop, a squelch, and each character's
//!   margin (how near a bit's phase change came to 90°).
//! * [`skim`]: candidates from the averaged spectrum, each confirmed as
//!   BPSK by the carrier line of the squared signal (which also gives the
//!   carrier exactly) and the symbol rate line of its envelope.
//! * [`synth`]: test signals.

pub mod demod;
pub mod skim;
pub mod synth;
pub mod varicode;

pub use skim::{Active, Event, Skimmer};
