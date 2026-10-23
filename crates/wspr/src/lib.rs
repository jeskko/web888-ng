// SPDX-License-Identifier: GPL-3.0-or-later
//! WSPR-2 for the web888d skimmers (`doc/m4-skimmers.md` §2.2): a Rust
//! port of WSJT-X's wsprd (GPL-3.0-or-later; Joe Taylor K1JT, Steven
//! Franke K9AN, and Phil Karn KA9Q's Fano decoder).

pub mod decode;
pub mod fano;
pub mod msg;
mod tables;

pub use decode::{Config, Decode, Decoder};
