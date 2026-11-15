// SPDX-License-Identifier: GPL-3.0-or-later
//! JS8 (JS8Call's keyboard mode: FT8's waveform, its own frames) for the
//! web UI. JS8Call (GPL-3.0) was read as the reference.
//!
//! * [`submode`]: Normal, Fast, Turbo and Slow.
//! * [`frames`]: a period of audio to frames (12 characters and a frame
//!   type each).
//! * [`message`]: a frame's bits to text (heartbeats, directed commands,
//!   compound callsigns, free text).
//! * [`jsc`]: the free text's dictionary compression.

pub mod frames;
pub mod jsc;
pub mod message;
pub mod submode;

pub use frames::{Frame, Frames};
pub use submode::Submode;
