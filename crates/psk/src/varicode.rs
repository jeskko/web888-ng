// SPDX-License-Identifier: GPL-3.0-or-later
//! G3PLX's Varicode (PSK31): a prefix-free-by-delimiter code where no
//! character contains two zeros in a row, so "00" separates characters.

/// The codes of ASCII 0-127, first bit first.
pub const TABLE: [&str; 128] = [
    "1010101011",
    "1011011011",
    "1011101101",
    "1101110111",
    "1011101011",
    "1101011111",
    "1011101111",
    "1011111101",
    "1011111111",
    "11101111",
    "11101",
    "1101101111",
    "1011011101",
    "11111",
    "1101110101",
    "1110101011",
    "1011110111",
    "1011110101",
    "1110101101",
    "1110101111",
    "1101011011",
    "1101101011",
    "1101101101",
    "1101010111",
    "1101111011",
    "1101111101",
    "1110110111",
    "1101010101",
    "1101011101",
    "1110111011",
    "1011111011",
    "1101111111",
    "1",
    "111111111",
    "101011111",
    "111110101",
    "111011011",
    "1011010101",
    "1010111011",
    "101111111",
    "11111011",
    "11110111",
    "101101111",
    "111011111",
    "1110101",
    "110101",
    "1010111",
    "110101111",
    "10110111",
    "10111101",
    "11101101",
    "11111111",
    "101110111",
    "101011011",
    "101101011",
    "110101101",
    "110101011",
    "110110111",
    "11110101",
    "110111101",
    "111101101",
    "1010101",
    "111010111",
    "1010101111",
    "1010111101",
    "1111101",
    "11101011",
    "10101101",
    "10110101",
    "1110111",
    "11011011",
    "11111101",
    "101010101",
    "1111111",
    "111111101",
    "101111101",
    "11010111",
    "10111011",
    "11011101",
    "10101011",
    "11010101",
    "111011101",
    "10101111",
    "1101111",
    "1101101",
    "101010111",
    "110110101",
    "101011101",
    "101110101",
    "101111011",
    "1010101101",
    "111110111",
    "111101111",
    "111111011",
    "1010111111",
    "101101101",
    "1011011111",
    "1011",
    "1011111",
    "101111",
    "101101",
    "11",
    "111101",
    "1011011",
    "101011",
    "1101",
    "111101011",
    "10111111",
    "11011",
    "111011",
    "1111",
    "111",
    "111111",
    "110111111",
    "10101",
    "10111",
    "101",
    "110111",
    "1111011",
    "1101011",
    "11011111",
    "1011101",
    "111010101",
    "1010110111",
    "110111011",
    "1010110101",
    "1011010111",
    "1110110101",
];

/// The bits of `text` (characters outside ASCII are skipped), each
/// character followed by "00".
pub fn encode(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for c in text.chars() {
        let Some(code) = TABLE.get(c as usize) else {
            continue;
        };
        out.extend(code.bytes().map(|b| b - b'0'));
        out.extend([0, 0]);
    }
    out
}

/// Bits to characters.
#[derive(Default)]
pub struct Decoder {
    code: u32,
    len: u32,
    zeros: u32,
}

impl Decoder {
    /// The next bit; a character when one ends ("00" after it). Codes not
    /// in the table (a bit error, or fldigi's extended codes) give U+FFFD;
    /// runs longer than any code give nothing.
    pub fn push(&mut self, bit: bool) -> Option<char> {
        if bit {
            // a single zero inside a code belongs to it; two or more were
            // a delimiter or idle reversals
            if self.zeros == 1 && self.len > 0 {
                self.add(false);
            }
            self.zeros = 0;
            self.add(true);
            return None;
        }
        self.zeros = self.zeros.saturating_add(1);
        if self.zeros == 2 && self.len > 0 {
            let (code, len) = (self.code, self.len);
            self.code = 0;
            self.len = 0;
            // no code is longer than 10 bits: a longer run was a steady
            // tone (or noise), not text
            return if len > 10 {
                None
            } else {
                Some(lookup(code, len).unwrap_or('\u{fffd}'))
            };
        }
        None
    }

    fn add(&mut self, bit: bool) {
        if self.len < 16 {
            self.code = (self.code << 1) | u32::from(bit);
            self.len += 1;
        }
    }
}

fn lookup(code: u32, len: u32) -> Option<char> {
    TABLE
        .iter()
        .position(|t| {
            t.len() as u32 == len
                && t.bytes().fold(0u32, |a, b| (a << 1) | u32::from(b - b'0')) == code
        })
        .map(|i| i as u8 as char)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_varicode() {
        for (i, t) in TABLE.iter().enumerate() {
            assert!(
                t.starts_with('1') && t.ends_with('1') && !t.contains("00"),
                "{i}: {t}"
            );
            assert!(
                TABLE.iter().filter(|u| u == &t).count() == 1,
                "{i}: {t} twice"
            );
        }
        assert_eq!(TABLE[b' ' as usize], "1");
        assert_eq!(TABLE[b'e' as usize], "11");
    }

    #[test]
    fn round_trip() {
        let text = "CQ CQ de N0CALL N0CALL pse k\r\n{|}~ 0123456789";
        let mut d = Decoder::default();
        // idle reversals before and after
        let bits: Vec<u8> = [vec![0u8; 20], encode(text), vec![0u8; 10]].concat();
        let s: String = bits.iter().filter_map(|&b| d.push(b == 1)).collect();
        assert_eq!(s, text);
    }
}
