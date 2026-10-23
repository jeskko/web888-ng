// SPDX-License-Identifier: GPL-3.0-or-later
//! FT8 and FT4 for the web888d skimmers (`doc/m4-skimmers.md` §2.1).
//!
//! The code tables, and the first version of the decoder's structure,
//! come from ft8_lib (MIT, Copyright (c) 2018 Kārlis Goba; RaspSDR's fork
//! in the stock server). The demodulator follows the published protocol
//! description (Franke, Somerville, Taylor: "The FT4 and FT8 Communication
//! Protocols", QEX July/August 2020).

pub mod crc;
pub mod ft8;
pub mod ldpc;
pub mod msg;
pub mod osd;
mod tables;
pub mod tones;
pub mod wav;
