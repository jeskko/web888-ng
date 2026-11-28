// SPDX-License-Identifier: GPL-3.0-or-later
//! Maritime DGPS beacons (283.5-325 kHz; ITU-R M.823): MSK at 100 or
//! 200 Bd (rarely 50) carrying RTCM SC-104 version 2 messages.
//! [`Skimmer`] finds every beacon in a passband of audio (or IQ) and
//! decodes it.
//!
//! * [`rtcm`]: 30-bit words with GPS parity (6 bits, the previous word's
//!   last two bits chained in), the two-word header (preamble 0x66, type,
//!   reference station, modified Z-count, sequence, length, health), and
//!   the message types worth showing: corrections (1, 9, 31: satellites),
//!   the reference station's position (3), the beacon almanac (7) and
//!   text (16).
//! * [`demod`]: one beacon at [`demod::SPS`] samples a bit: the phase
//!   turned over one bit (±90° for MSK) gives the bit, bit timing from
//!   where those turns are largest, a slow frequency loop on the doubled
//!   turn.
//! * [`skim`]: candidates from the squared signal (MSK squares to two
//!   lines, a bit rate apart, centred on twice the carrier), kept once
//!   their words pass parity.
//! * [`synth`]: test signals.

pub mod demod;
pub mod rtcm;
pub mod skim;
pub mod synth;

pub use rtcm::{Body, Msg};
pub use skim::{Active, Event, Skimmer};
