// SPDX-License-Identifier: GPL-3.0-or-later
//! HFDL's physical layer (ARINC 635; the details as dumphfdl, GPL-3.0,
//! implements them): 1800 Bd PSK at 1440 Hz above the listed (USB dial)
//! frequency. A burst is
//!
//! | part | symbols | |
//! |---|---|---|
//! | prekey | 448 | |
//! | A, A | 2 × 127 | BPSK, [`A`] |
//! | M1 | 127 | BPSK, [`M1`] rotated by the mode's shift: rate and slot |
//! | M2 | 15 | the first 15 of the same |
//! | T × 9 | 9 × 15 | BPSK, [`T`] |
//! | frames | 72 or 168 × (30 + 15) | 30 data symbols, then a [`T`] probe |
//!
//! BPSK symbols: bit 0 is +1, bit 1 is −1. Data symbols are BPSK, QPSK or
//! 8PSK (Gray coded, MSB first, symbol s at 2πs/M), each turned by 180°
//! where the scrambler ([`scrambler`], 120 bits, restarting each burst)
//! gives a one. The bits go through a 40-row block interleaver
//! ([`interleave`]) after a K=7 rate 1/2 convolutional code ([`viterbi`]);
//! 300 bps sends each coded bit twice.

/// Symbols a second.
pub const BAUD: f64 = 1800.0;
/// The burst's centre above the listed frequency (Hz).
pub const CENTRE: f64 = 1440.0;
pub const PREKEY: usize = 448;
pub const A_LEN: usize = 127;
pub const M1_LEN: usize = 127;
pub const M2_LEN: usize = 15;
pub const T_LEN: usize = 15;
pub const T_PREAMBLE: usize = 9;
pub const DATA: usize = 30;
/// Symbols from the first A to the first data symbol.
pub const PREAMBLE: usize = 2 * A_LEN + M1_LEN + M2_LEN + T_PREAMBLE * T_LEN;

const A_OCTETS: [u8; 16] = [
    0b01011011, 0b10111100, 0b01110100, 0b01010111, 0b00000011, 0b11011001, 0b10001001, 0b00111001,
    0b11110010, 0b00001000, 0b11010101, 0b00110110, 0b10010100, 0b00101100, 0b00110010, 0b11111110,
];

/// The A sequence's bits.
pub fn a() -> [u8; A_LEN] {
    std::array::from_fn(|i| (A_OCTETS[i / 8] >> (7 - i % 8)) & 1)
}

const M1_BITS: [u8; M1_LEN] = [
    0, 1, 1, 1, 0, 1, 1, 0, 1, 1, 1, 1, 0, 1, 0, 0, 0, 1, 0, 1, 1, 0, 0, 1, 0, 1, 1, 1, 1, 1, 0, 0,
    0, 1, 0, 0, 0, 0, 0, 0, 1, 1, 0, 0, 1, 1, 0, 1, 1, 0, 0, 0, 1, 1, 1, 0, 0, 1, 1, 1, 0, 1, 0, 1,
    1, 1, 0, 0, 0, 0, 1, 0, 0, 1, 1, 0, 0, 0, 0, 0, 1, 0, 1, 0, 1, 0, 1, 1, 0, 1, 0, 0, 1, 0, 0, 1,
    0, 1, 0, 0, 1, 1, 1, 1, 0, 0, 1, 0, 0, 0, 1, 1, 0, 1, 0, 1, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1,
];

/// The training (probe) sequence's bits, 0x9AF MSB first.
pub const T: [u8; T_LEN] = [0, 0, 0, 1, 0, 0, 1, 1, 0, 1, 0, 1, 1, 1, 1];

/// One of the eight burst types M1 announces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mode {
    /// 0-7 (M1's rotation).
    pub index: usize,
    /// Bits a data symbol: 1 (BPSK), 2 (QPSK), 3 (8PSK).
    pub bits: usize,
    /// Each coded bit sent twice (300 bps).
    pub repeat: bool,
    /// Data frames: 72 (single slot) or 168 (double).
    pub frames: usize,
}

const SHIFTS: [usize; 8] = [72, 82, 113, 123, 61, 103, 93, 9];

impl Mode {
    pub fn new(index: usize) -> Mode {
        let (bits, repeat) = [(1, true), (1, false), (2, false), (3, false)][index % 4];
        Mode {
            index,
            bits,
            repeat,
            frames: if index < 4 { 72 } else { 168 },
        }
    }

    /// User bits a second (data symbols only: the probes cost a third).
    pub fn bps(&self) -> u32 {
        [300, 600, 1200, 1800][self.index % 4]
    }

    pub fn double(&self) -> bool {
        self.frames == 168
    }

    /// M1 (and the first 15 bits of it, M2).
    pub fn m1(&self) -> [u8; M1_LEN] {
        std::array::from_fn(|j| M1_BITS[(SHIFTS[self.index] + j) % M1_LEN])
    }

    pub fn data_symbols(&self) -> usize {
        self.frames * DATA
    }

    /// Symbols from the first A to the burst's end.
    pub fn symbols(&self) -> usize {
        PREAMBLE + self.frames * (DATA + T_LEN)
    }

    /// Coded bits (interleaver size).
    pub fn coded(&self) -> usize {
        self.data_symbols() * self.bits
    }

    /// Decoded (user) bits.
    pub fn user_bits(&self) -> usize {
        self.coded() / if self.repeat { 4 } else { 2 }
    }

    fn column_shift(&self) -> usize {
        if self.double() { 23 } else { 17 }
    }
}

/// The 120-bit scrambler sequence: a 15-bit LFSR (taps 0x4001) from 0x4d4b
/// (dumphfdl with liquid-dsp 1.6+; confirmed on air: the bits common to
/// many bursts, zero padding, repeat with it).
pub fn scrambler() -> [u8; 120] {
    let mut v: u32 = 0x4d4b;
    std::array::from_fn(|_| {
        let b = ((v & 0x4001).count_ones() & 1) as u8;
        v = ((v << 1) | b as u32) & 0x7fff;
        b
    })
}

const ROWS: usize = 40;

/// The interleaver as a permutation: element i of the deinterleaved
/// (decoder's) order is element `p[i]` of the received order.
pub fn interleave(mode: &Mode) -> Vec<u32> {
    let n = mode.coded();
    let cols = n / ROWS;
    let shift = mode.column_shift();
    let mut at = vec![u32::MAX; n];
    let (mut row, mut col) = (0usize, 0usize);
    for i in 0..n {
        at[row * cols + col] = i as u32;
        row += 1;
        if row == ROWS {
            row = 0;
            col += 1;
        }
        col = (col + cols - shift % cols) % cols;
    }
    let (mut row, mut col) = (0usize, 0usize);
    (0..n)
        .map(|_| {
            let v = at[row * cols + col];
            row = (row + 9) % ROWS;
            if row == 0 {
                col += 1;
            }
            v
        })
        .collect()
}

/// The convolutional code's generators (K = 7; the register's newest bit
/// is bit 0), in the order sent (confirmed on air: a strong burst
/// re-encodes exactly).
pub const POLY: [u32; 2] = [0x6d, 0x4f];

/// Encodes bits (for tests), two coded bits each.
pub fn encode(bits: &[u8]) -> Vec<u8> {
    let mut sr = 0u32;
    let mut out = Vec::with_capacity(2 * bits.len());
    for &b in bits {
        sr = ((sr << 1) | b as u32) & 0x7f;
        for p in POLY {
            out.push(((sr & p).count_ones() & 1) as u8);
        }
    }
    out
}

/// Soft Viterbi decoding: `llr` two per bit, positive for 0. Starts in
/// state 0, ends in the best.
pub fn viterbi(llr: &[f32]) -> Vec<u8> {
    let n = llr.len() / 2;
    // expected output bits per 7-bit register (old state << 1 | input)
    let out: [[u8; 2]; 128] =
        std::array::from_fn(|r| POLY.map(|p| ((r as u32 & p).count_ones() & 1) as u8));
    let mut metric = [f32::NEG_INFINITY; 64];
    metric[0] = 0.0;
    let mut from = vec![[0u8; 64]; n];
    for (k, step) in from.iter_mut().enumerate() {
        let (l0, l1) = (llr[2 * k], llr[2 * k + 1]);
        let mut next = [f32::NEG_INFINITY; 64];
        for (s, nm) in next.iter_mut().enumerate() {
            // new state s = (old << 1 | b) & 63: b = s & 1, old = s >> 1 or that + 32
            for old in [s >> 1, (s >> 1) | 32] {
                let r = (old << 1) | (s & 1);
                let o = out[r];
                let m = metric[old]
                    + if o[0] == 0 { l0 } else { -l0 }
                    + if o[1] == 0 { l1 } else { -l1 };
                if m > *nm {
                    *nm = m;
                    step[s] = old as u8;
                }
            }
        }
        // keep the numbers small
        let top = next.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        for (m, v) in metric.iter_mut().zip(next) {
            *m = v - top;
        }
    }
    let mut s = (0..64)
        .max_by(|&a, &b| metric[a].total_cmp(&metric[b]))
        .unwrap_or(0);
    let mut bits = vec![0u8; n];
    for k in (0..n).rev() {
        bits[k] = (s & 1) as u8;
        s = from[k][s] as usize;
    }
    bits
}

/// Bits to octets, the first bit the least significant.
pub fn octets(bits: &[u8]) -> Vec<u8> {
    bits.chunks(8)
        .map(|c| c.iter().enumerate().fold(0u8, |a, (i, &b)| a | (b << i)))
        .collect()
}

/// CRC-16/X.25 (CCITT reflected, init and final XOR 0xFFFF).
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0xffffu16;
    for &d in data {
        crc ^= d as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0x8408
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// The CRC of the first `n` octets checks against the two after them
/// (least significant first).
pub fn fcs_ok(buf: &[u8], n: usize) -> bool {
    buf.len() >= n + 2 && crc16(&buf[..n]) == u16::from_le_bytes([buf[n], buf[n + 1]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleaver_is_a_permutation() {
        for m in 0..8 {
            let p = interleave(&Mode::new(m));
            let mut seen = vec![false; p.len()];
            for &i in &p {
                assert!(!seen[i as usize], "mode {m}");
                seen[i as usize] = true;
            }
        }
    }

    #[test]
    fn viterbi_round_trip() {
        let bits: Vec<u8> = (0..500u32).map(|i| ((i * 7919) >> 3 & 1) as u8).collect();
        let mut llr: Vec<f32> = encode(&bits)
            .iter()
            .map(|&c| if c == 0 { 1.0 } else { -1.0 })
            .collect();
        // a few errors
        for i in [5, 77, 200, 431, 800] {
            llr[i] = -llr[i];
        }
        assert_eq!(viterbi(&llr), bits);
    }

    #[test]
    fn crc() {
        // CRC-16/X-25 check value
        assert_eq!(crc16(b"123456789"), 0x906e);
    }

    #[test]
    fn sequences() {
        // the A sequence: 127 bits, the last octet's low bit unused
        assert_eq!(a()[126], 1);
        assert_eq!(Mode::new(3).m1()[0], M1_BITS[123]);
        assert_eq!(PREAMBLE, 531);
        assert_eq!(Mode::new(0).user_bits(), 540);
        assert_eq!(Mode::new(7).user_bits(), 7560);
    }
}
