// SPDX-License-Identifier: GPL-3.0-or-later
//! JS8Call's text compression (`jsc.cpp`): each word (or its prefix) is
//! an index into a 262144-entry dictionary (`data/jsc_words.txt`, from
//! JS8Call's `jsc_map.cpp`, one entry per line, `\n` and `\\` escaped),
//! written as a (7,9)-dense code of 4-bit digits; the last digit of an
//! index (under 7) is followed by a bit saying a space comes after the
//! word.

use std::sync::OnceLock;

static WORDS_TEXT: &str = include_str!("../data/jsc_words.txt");

/// Where each entry starts in the text (and the end of the last).
fn offsets() -> &'static [u32] {
    static O: OnceLock<Vec<u32>> = OnceLock::new();
    O.get_or_init(|| {
        let mut o = vec![0u32];
        o.extend(WORDS_TEXT.match_indices('\n').map(|(i, _)| i as u32 + 1));
        o
    })
}

/// Entries in the dictionary.
pub fn size() -> usize {
    offsets().len() - 1
}

/// Entry `i` (escapes undone).
pub fn word(i: usize) -> std::borrow::Cow<'static, str> {
    let o = offsets();
    let l = &WORDS_TEXT[o[i] as usize..o[i + 1] as usize - 1];
    if l.contains('\\') {
        l.replace("\\n", "\n").replace("\\\\", "\\").into()
    } else {
        l.into()
    }
}

const S: u32 = 7;
const C: u32 = 16 - S;

/// The text of compressed bits, or None if they don't make any.
pub fn decompress(bits: &[bool]) -> Option<String> {
    let size = size() as u32;
    let mut base = [0u32; 8];
    let mut p = S;
    for k in 1..8 {
        base[k] = base[k - 1] + p;
        p *= C;
    }
    let mut digits: Vec<u32> = Vec::new();
    let mut separators: Vec<usize> = Vec::new();
    let mut i = 0;
    while i + 4 <= bits.len() {
        let d = bits[i..i + 4].iter().fold(0u32, |a, &b| a << 1 | b as u32);
        digits.push(d);
        i += 4;
        if d < S {
            if i < bits.len() && bits[i] {
                separators.push(digits.len() - 1);
            }
            i += 1;
        }
    }
    let mut out = String::new();
    let mut sep = separators.into_iter().peekable();
    let mut start = 0;
    while start < digits.len() {
        let (mut k, mut j) = (0, 0u32);
        while start + k < digits.len() && digits[start + k] >= S {
            j = j.saturating_mul(C).saturating_add(digits[start + k] - S);
            k += 1;
        }
        if j >= size || start + k >= digits.len() || k >= base.len() {
            break;
        }
        let j = j * S + digits[start + k] + base[k];
        if j >= size {
            break;
        }
        out += &word(j as usize);
        if sep.peek() == Some(&(start + k)) {
            out.push(' ');
            sep.next();
        }
        start += k + 1;
    }
    (!out.is_empty()).then_some(out)
}

/// Compresses text whose words are all whole dictionary entries
/// (tests; JS8Call also splits words into prefixes).
pub fn compress(text: &str) -> Option<Vec<bool>> {
    let mut base = [0u32; 8];
    let mut p = S;
    for k in 1..8 {
        base[k] = base[k - 1] + p;
        p *= C;
    }
    let parts: Vec<&str> = text.split(' ').collect();
    let mut out = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let idx = (0..size()).position(|i| word(i) == *part)? as u32;
        // k leading digits (7..15), then the last (0..6)
        let k = (0..7).rev().find(|&k| idx >= base[k])?;
        let r = idx - base[k];
        let mut x = r / S;
        let mut digits = vec![0u32; k];
        for d in digits.iter_mut().rev() {
            *d = x % C + S;
            x /= C;
        }
        digits.push(r % S);
        for d in digits {
            out.extend((0..4).rev().map(|b| d >> b & 1 == 1));
        }
        out.push(i + 1 < parts.len());
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictionary() {
        assert_eq!(size(), 262144);
        let w: Vec<String> = [0, 67, 68, 69, 70]
            .iter()
            .map(|&i| word(i).into_owned())
            .collect();
        assert_eq!(w, ["E", " ", "\\", "\n", "JS8"]);
    }

    #[test]
    fn round_trip() {
        for t in ["HELLO WORLD", "JS8CALL QSO 73 TNX", "E", "QUICK BROWN FOX"] {
            let b = compress(t).unwrap();
            assert_eq!(decompress(&b).as_deref(), Some(t), "{t}");
        }
    }
}
