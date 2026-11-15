// SPDX-License-Identifier: GPL-3.0-or-later
//! HFDL (ARINC 635, the HF data link aircraft and 16 ground stations use
//! for ACARS and position reports): 1800 Bd PSK bursts in 2.4 s TDMA slots
//! on frequencies from 2.9 to 22 MHz, the burst's centre 1440 Hz above
//! the listed frequency (USB).
//!
//! * [`front`]: audio or IQ to complex baseband at 4 samples a symbol,
//!   matched filtered.
//! * [`demod`]: burst detection, frequency, an equaliser, the burst's
//!   type (M1), soft bits, deinterleaving and Viterbi decoding.
//! * [`phy`]: the sequences, scrambler, interleaver, code and CRC.
//! * [`pdu`]: octets to messages (squitters, logons, position reports,
//!   ACARS), each part checked by its CRC.
//! * [`stations`]: the ground stations and their frequencies.
//! * [`skim`]: one channel from samples to described messages, with what
//!   is known of each aircraft filled in.
//! * [`synth`]: test bursts.

pub mod demod;
pub mod front;
pub mod pdu;
pub mod phy;
pub mod skim;
pub mod stations;
pub mod synth;
