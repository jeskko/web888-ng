// SPDX-License-Identifier: GPL-3.0-or-later
//! JS8 frames (JS8Call, GPL-3.0, read as the reference): FT8's 79-symbol
//! 8-FSK at its timing, with
//!
//! * 87 bits: 72 text bits (12 characters of [`ALPHABET`], 6 bits each),
//!   a 3-bit frame type, and a CRC-12 (boost's augmented CRC, polynomial
//!   0xc06, over the 75 bits and 13 zeros, XOR 42);
//! * the (174,87) LDPC code ([`crate::js8code`]): a codeword is the 87
//!   parity bits, then the 87 message bits;
//! * tones straight from 3-bit words (no Gray code): the parity in symbols
//!   7-35, the message in 43-71, Costas arrays at 0, 36 and 72 (the
//!   original array three times for Normal, three others for the faster
//!   and slower submodes);
//! * plain FSK, no Gaussian shaping.

use crate::js8code::PARITY;
use crate::ldpc::{Bits, N, get, set};

/// The text characters, a 6-bit word each.
pub const ALPHABET: &[u8; 64] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz-+";

/// Normal's Costas arrays (the original one, three times).
pub const COSTAS_NORMAL: [[u8; 7]; 3] = [[4, 2, 5, 6, 1, 3, 0]; 3];
/// The other submodes'.
pub const COSTAS_MODIFIED: [[u8; 7]; 3] = [
    [0, 6, 2, 3, 5, 4, 1],
    [1, 5, 0, 2, 3, 6, 4],
    [2, 5, 0, 6, 4, 1, 3],
];
/// Tones are the 3-bit words themselves.
pub const TONE_MAP: [u8; 8] = [0, 1, 2, 3, 4, 5, 6, 7];

pub const K: usize = 87;

/// CRC-12 of the 75 bits (`m75`, the first sent at bit 74).
pub fn crc12(m75: u128) -> u16 {
    // 75 bits, then 13 zeros (the CRC's place and a spare bit): 11 octets
    let a = m75 << 13;
    let mut r: u16 = 0;
    for i in (0..88).rev() {
        let top = r >> 11 & 1;
        r = ((r << 1) | (a >> i & 1) as u16) & 0xfff;
        if top == 1 {
            r ^= 0xc06;
        }
    }
    r ^ 42
}

/// The 87 bits: message and CRC.
pub fn with_crc(m75: u128) -> u128 {
    (m75 << 12) | crc12(m75) as u128
}

/// The 75 message bits if the CRC holds.
pub fn check(a87: u128) -> Option<u128> {
    let m = a87 >> 12;
    (crc12(m) == (a87 & 0xfff) as u16).then_some(m)
}

fn parity_bit(row: usize, j: usize) -> bool {
    let d = PARITY[row].as_bytes()[j / 4];
    let v = (d as char).to_digit(16).unwrap_or(0);
    v >> (3 - j % 4) & 1 != 0
}

/// Parity row `i` as an 87-bit word (message bit 0 at bit 86).
fn rows() -> &'static [u128; K] {
    static ROWS: std::sync::OnceLock<[u128; K]> = std::sync::OnceLock::new();
    ROWS.get_or_init(|| {
        std::array::from_fn(|i| (0..K).fold(0u128, |r, j| r << 1 | parity_bit(i, j) as u128))
    })
}

/// Encodes the 87 bits.
pub fn encode(a87: u128) -> Bits {
    let mut c = [0u64; 3];
    for (i, row) in rows().iter().enumerate() {
        set(&mut c, i, (row & a87).count_ones() & 1 != 0);
    }
    for j in 0..K {
        set(&mut c, K + j, a87 >> (K - 1 - j) & 1 != 0);
    }
    c
}

/// The 87 message bits of a codeword.
pub fn message(c: &Bits) -> u128 {
    (K..N).fold(0u128, |a, j| a << 1 | get(c, j) as u128)
}

/// The 79 tones of 87 bits.
pub fn tones(a87: u128, costas: &[[u8; 7]; 3]) -> [u8; 79] {
    let c = encode(a87);
    let mut t = [0u8; 79];
    for (a, arr) in costas.iter().enumerate() {
        t[36 * a..36 * a + 7].copy_from_slice(arr);
    }
    for s in 0..58 {
        let k = if s < 29 { 7 + s } else { 43 + s - 29 };
        t[k] = (0..3).fold(0, |v, b| v << 1 | get(&c, 3 * s + b) as u8);
    }
    t
}

/// The 12 characters and the frame type of 75 bits.
pub fn text(m75: u128) -> (String, u8) {
    let s = (0..12)
        .map(|i| ALPHABET[(m75 >> (3 + 6 * (11 - i)) & 63) as usize] as char)
        .collect();
    (s, (m75 & 7) as u8)
}

/// 75 bits from 12 characters and a frame type (`None`: a character not
/// in the alphabet).
pub fn pack(text: &str, kind: u8) -> Option<u128> {
    let b = text.as_bytes();
    if b.len() != 12 {
        return None;
    }
    let mut m = 0u128;
    for &c in b {
        m = m << 6 | ALPHABET.iter().position(|&a| a == c)? as u128;
    }
    Some(m << 3 | (kind & 7) as u128)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldpc::Bp;

    #[test]
    fn codewords_and_bp() {
        let bp_errors = Bp::js8();
        for (i, t) in ["HELLOWORLD12", "0123456789ab", "zzzzzzzzzzzz"]
            .iter()
            .enumerate()
        {
            let m = pack(t, i as u8 + 1).unwrap();
            let a = with_crc(m);
            let c = encode(a);
            assert_eq!(bp_errors.errors(&c), 0, "{t}");
            assert_eq!(check(message(&c)), Some(m));
            assert_eq!(text(m), (t.to_string(), i as u8 + 1));
            // a weakened codeword decodes back
            let mut llr: [f32; N] = std::array::from_fn(|k| if get(&c, k) { 3.0 } else { -3.0 });
            for k in (0..N).step_by(11) {
                llr[k] *= -0.3;
            }
            let d = Bp::js8().decode(&llr, 30);
            assert_eq!((d.errors, d.codeword), (0, c));
        }
        // a flipped bit breaks the CRC
        let a = with_crc(pack("HELLOWORLD12", 0).unwrap());
        assert_eq!(check(a ^ 1 << 40), None);
    }

    #[test]
    fn tones_layout() {
        let t = tones(with_crc(pack("CQCQCQ000000", 0).unwrap()), &COSTAS_MODIFIED);
        assert_eq!(&t[..7], &COSTAS_MODIFIED[0]);
        assert_eq!(&t[36..43], &COSTAS_MODIFIED[1]);
        assert_eq!(&t[72..], &COSTAS_MODIFIED[2]);
        assert!(t.iter().all(|&v| v < 8));
    }
}
