// SPDX-License-Identifier: GPL-3.0-or-later
//! FT8 and FT4 for the web888d skimmers (`doc/m4-skimmers.md` §2.1).
//!
//! The code tables, and the first version of the decoder's structure,
//! come from ft8_lib (MIT, Copyright (c) 2018 Kārlis Goba; RaspSDR's fork
//! in the stock server). The demodulator follows the published protocol
//! description (Franke, Somerville, Taylor: "The FT4 and FT8 Communication
//! Protocols", QEX July/August 2020).

pub mod crc;
pub mod ft4;
pub mod ft8;
pub mod js8;
pub mod js8code;
pub mod ldpc;
pub mod msg;
pub mod osd;
mod realfft;
mod tables;
pub mod tones;
pub mod wav;

use num_complex::Complex32 as C32;
use rustfft::Fft;
use std::cell::RefCell;

/// One scratch buffer for a decoder's FFTs: rustfft's `process`
/// allocates (and on musl, maps and zeroes) a new one every call.
#[derive(Default)]
pub(crate) struct FftScratch(RefCell<Vec<C32>>);

impl FftScratch {
    /// `fft` over `buf` in place, as `fft.process(buf)`.
    pub(crate) fn run(&self, fft: &dyn Fft<f32>, buf: &mut [C32]) {
        let n = fft.get_inplace_scratch_len();
        let mut s = self.0.borrow_mut();
        if s.len() < n {
            s.resize(n, C32::default());
        }
        fft.process_with_scratch(buf, &mut s[..n]);
    }
}

/// `i` mod `n` for an `i` within a few `n` of 0: no 64-bit division (a
/// library call on the A9).
#[inline]
pub(crate) fn wrap(i: i64, n: usize) -> usize {
    let n = n as i64;
    let mut i = i;
    while i < 0 {
        i += n;
    }
    while i >= n {
        i -= n;
    }
    i as usize
}
