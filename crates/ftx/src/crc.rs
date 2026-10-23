// SPDX-License-Identifier: GPL-3.0-or-later
//! CRC-14 of FT8/FT4 (polynomial 0x2757): computed over the 77 message
//! bits followed by 5 zero bits, and sent after them (91 bits in all).

const POLY: u16 = 0x2757;

/// CRC of the 77-bit message `m` (bit 76 is sent first).
pub fn crc14(m: u128) -> u16 {
    let v = m << 5; // 82 bits
    let mut r: u16 = 0;
    for i in (0..82).rev() {
        let bit = ((v >> i) & 1) as u16;
        let top = (r >> 13) & 1;
        r = ((r << 1) | bit) & 0x3fff;
        if top != 0 {
            r ^= POLY;
        }
    }
    // Flush the register through 14 more zero bits.
    for _ in 0..14 {
        let top = (r >> 13) & 1;
        r = (r << 1) & 0x3fff;
        if top != 0 {
            r ^= POLY;
        }
    }
    r
}

/// The 91 bits sent: message and CRC.
pub fn with_crc(m: u128) -> u128 {
    (m << 14) | crc14(m) as u128
}

/// The message of a 91-bit word whose CRC matches.
pub fn check(a91: u128) -> Option<u128> {
    let m = a91 >> 14;
    (crc14(m) as u128 == a91 & 0x3fff).then_some(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_ft8code() {
        // ft8code "CQ K1ABC FN42"
        let m = u128::from_str_radix(
            "00000000000000000000000000100000010011011110111100011010100010100001100110001",
            2,
        )
        .unwrap();
        assert_eq!(crc14(m), 0b00101100101110);
        assert_eq!(check(with_crc(m)), Some(m));
        assert_eq!(check(with_crc(m) ^ 1 << 40), None);
    }
}
