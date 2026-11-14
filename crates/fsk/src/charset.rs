// SPDX-License-Identifier: GPL-3.0-or-later
//! Teleprinter character sets: ITA2 (Baudot; international and US-TTY
//! figures, unshift-on-space decided from the text), CCIR 476 (SITOR /
//! NAVTEX), ASCII; and a plausibility score for decoded text.

/// ITA2 by code value (first data bit = LSB): letters, ITA2 figures,
/// US-TTY figures.
pub const ITA2_LTRS: &[u8; 32] = b"\0E\nA SIU\rDRJNFCKTZLWHYPQOBG\x0eMXV\x0f";
pub const ITA2_FIGS: &[u8; 32] = b"\x003\n- '87\r\x054\x07,!:(5+)2#6019?&\x0e./=\x0f";
pub const US_FIGS: &[u8; 32] = b"\x003\n- \x0787\r$4',!:(5\")2#6019?&\x0e./;\x0f";
pub const FIGS: u32 = 27;
pub const LTRS: u32 = 31;
pub const SPACE: u32 = 4;

/// CCIR 476 (7-bit, four marks): code → (letter, figure).
pub const C476: &[(u32, u8, u8)] = &[
    (0x17, b'J', b'\''),
    (0x1b, b'F', b'!'),
    (0x1d, b'C', b':'),
    (0x1e, b'K', b'('),
    (0x27, b'W', b'2'),
    (0x2b, b'Y', b'6'),
    (0x2d, b'P', b'0'),
    (0x2e, b'Q', b'1'),
    (0x35, b'G', b'&'),
    (0x39, b'M', b'.'),
    (0x3a, b'X', b'/'),
    (0x3c, b'V', b';'),
    (0x47, b'A', b'-'),
    (0x4b, b'S', 0x07),
    (0x4d, b'I', b'8'),
    (0x4e, b'U', b'7'),
    (0x53, b'D', b'$'),
    (0x55, b'R', b'4'),
    (0x56, b'E', b'3'),
    (0x59, b'N', b','),
    (0x5c, b' ', b' '),
    (0x63, b'Z', b'"'),
    (0x65, b'L', b')'),
    (0x69, b'H', b'#'),
    (0x6c, b'\n', b'\n'),
    (0x71, b'O', b'9'),
    (0x72, b'B', b'?'),
    (0x74, b'T', b'5'),
    (0x78, b'\r', b'\r'),
];
pub const C476_LTRS: u32 = 0x5a;
pub const C476_FIGS: u32 = 0x36;
pub const C476_ALPHA: u32 = 0x0f;
pub const C476_BETA: u32 = 0x33;
pub const C476_REP: u32 = 0x66;
pub const C476_C32: u32 = 0x6a;

/// Seven bits with exactly four marks (a CCIR 476 word).
pub fn w4(c: u32) -> bool {
    c < 128 && c.count_ones() == 4
}

/// A character as shown: newline kept, other control characters dropped.
fn printable(c: u8) -> Option<char> {
    match c {
        b'\n' => Some('\n'),
        0x20..=0x7e => Some(c as char),
        _ => None,
    }
}

/// ITA2 decoder state (letters/figures shift).
#[derive(Clone, Debug)]
pub struct Ita2 {
    pub usos: bool,
    figs: bool,
    fig_tab: &'static [u8; 32],
}

impl Ita2 {
    pub fn new(usos: bool, us: bool) -> Self {
        Ita2 {
            usos,
            figs: false,
            fig_tab: if us { US_FIGS } else { ITA2_FIGS },
        }
    }

    pub fn char(&mut self, code: u32) -> Option<char> {
        let code = code & 31;
        if code == FIGS {
            self.figs = true;
            return None;
        }
        if code == LTRS {
            self.figs = false;
            return None;
        }
        if code == SPACE && self.usos {
            self.figs = false;
        }
        printable((if self.figs { self.fig_tab } else { ITA2_LTRS })[code as usize])
    }
}

/// Consonant-only words normal in teleprinter traffic.
const ABBREV: &[&str] = &[
    "CQ", "KHZ", "MHZ", "HZ", "UTC", "GMT", "MSG", "NR", "TST", "ZCZC", "NNNN", "N", "S", "W", "E",
    "NW", "SW", "NNW", "NNE", "SSW", "SSE", "WNW", "WSW", "ENE", "ESE", "CW", "RTTY", "TNX", "PSE",
    "BK", "KN", "SK", "HW", "FB", "WX", "RST", "TX", "RX", "DX", "GL", "GM", "PWR", "FM",
];

/// Plausibility of a teleprinter word (upper case), 0..1: numbers with
/// number punctuation, calls and locators, separators, Q codes, or letters
/// with a vowel and no Q without U; short consonant-only words count half.
pub fn word_plaus(w: &str) -> f64 {
    let s = w.trim_end_matches(['.', ',', ':', ';', '?', '\'', '"']);
    if s.is_empty() || s.chars().all(|c| "/-=.+".contains(c)) {
        return f64::from(u8::from(w.chars().all(|c| "/-=.+.,:".contains(c))));
    }
    let w = s;
    if w.chars().any(|c| c.is_ascii_digit()) {
        let first = w.chars().next().unwrap_or(' ');
        let last = w.chars().last().unwrap_or(' ');
        if w.chars()
            .all(|c| c.is_ascii_digit() || ".,:/-+=".contains(c))
            && (first.is_ascii_digit() || first == '+' || first == '-')
            && last.is_ascii_digit()
        {
            return 1.0;
        }
        return f64::from(u8::from(
            w.chars().count() <= 10
                && w.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '/'),
        ));
    }
    if !w.chars().all(char::is_alphabetic) {
        return 0.0;
    }
    let b: Vec<char> = w.chars().collect();
    if ABBREV.contains(&w)
        || (b.len() == 3 && b[0] == 'Q' && b[1].is_ascii_uppercase() && b[2].is_ascii_uppercase())
    {
        return 1.0;
    }
    if b.iter()
        .enumerate()
        .any(|(i, &c)| c == 'Q' && b.get(i + 1) != Some(&'U'))
    {
        return 0.0;
    }
    if !b.iter().any(|c| "AEIOUY".contains(*c)) {
        return if b.len() <= 3 { 0.5 } else { 0.0 };
    }
    1.0
}

/// ITA2 with unshift-on-space decided from the text: both variants decode
/// every code, and words where they differ vote (DWD keeps figures across
/// spaces, ham RTTY unshifts).
#[derive(Clone, Debug)]
pub struct Ita2Auto {
    dec: [Ita2; 2], // [usos off, usos on]
    pub usos: bool,
    word: [String; 2],
    vote: f64,
}

impl Ita2Auto {
    pub fn new(usos: bool, us: bool) -> Self {
        Ita2Auto {
            dec: [Ita2::new(false, us), Ita2::new(true, us)],
            usos,
            word: Default::default(),
            vote: 0.0,
        }
    }

    pub fn char(&mut self, code: u32) -> Option<char> {
        let code = code & 31;
        let out = [self.dec[0].char(code), self.dec[1].char(code)];
        if code == SPACE || code == 2 || code == 8 {
            let (a, b) = (
                &self.word[usize::from(self.usos)],
                &self.word[usize::from(!self.usos)],
            );
            if a != b {
                self.vote += word_plaus(b) - word_plaus(a);
                if self.vote >= 1.5 {
                    self.usos = !self.usos;
                    self.vote = 0.0;
                } else if self.vote < -3.0 {
                    self.vote = -3.0;
                }
            }
            self.word = Default::default();
        } else {
            for (w, o) in self.word.iter_mut().zip(out) {
                if let Some(c) = o {
                    w.push(c);
                }
            }
        }
        out[usize::from(self.usos)]
    }
}

/// The unshift-on-space setting the text of these codes supports.
pub fn ita2_usos(codes: &[u32], us: bool) -> bool {
    let mut a = Ita2Auto::new(true, us);
    for &c in codes {
        a.char(c);
    }
    a.usos
}

/// CCIR 476 decoder state.
#[derive(Clone, Debug, Default)]
pub struct Ccir476 {
    figs: bool,
}

impl Ccir476 {
    pub fn char(&mut self, code: u32) -> Option<char> {
        if code == C476_FIGS {
            self.figs = true;
            return None;
        }
        if code == C476_LTRS {
            self.figs = false;
            return None;
        }
        let e = C476.iter().find(|e| e.0 == code)?;
        printable(if self.figs { e.2 } else { e.1 })
    }
}

/// An ASCII character (7 or 8 bits, LSB or MSB first).
pub fn ascii_char(code: u32, nbits: u32, msb_first: bool) -> Option<char> {
    let code = if msb_first {
        code.reverse_bits() >> (32 - nbits)
    } else {
        code
    };
    if code == 10 {
        return Some('\n');
    }
    if (32..127).contains(&code) || (code >= 160 && nbits == 8) {
        char::from_u32(code)
    } else {
        None
    }
}

/// Text → ITA2 codes with LTRS/FIGS shifts (starts with LTRS).
pub fn ita2_encode(text: &str, usos: bool) -> Vec<u32> {
    let mut out = vec![LTRS];
    let mut figs = false;
    let find = |tab: &[u8; 32], c: u8| tab.iter().position(|&x| x == c).map(|i| i as u32);
    for c in text.to_uppercase().bytes() {
        if c == b'\n' {
            out.extend([8, 2]);
            continue;
        }
        if c == b' ' {
            out.push(SPACE);
            if usos {
                figs = false;
            }
            continue;
        }
        if let Some(i) = find(ITA2_LTRS, c).filter(|_| c.is_ascii_uppercase()) {
            if figs {
                out.push(LTRS);
                figs = false;
            }
            out.push(i);
        } else if let Some(i) = find(ITA2_FIGS, c).filter(|_| c >= 0x20) {
            if !figs {
                out.push(FIGS);
                figs = true;
            }
            out.push(i);
        }
    }
    out
}

/// Text → CCIR 476 codes.
pub fn ccir476_encode(text: &str) -> Vec<u32> {
    let mut out = vec![C476_LTRS];
    let mut figs = false;
    for c in text.to_uppercase().bytes() {
        if c == b'\n' {
            out.extend([0x78, 0x6c]);
        } else if c == b' ' {
            out.push(0x5c);
        } else if let Some(e) = C476.iter().find(|e| e.1 == c) {
            if figs {
                out.push(C476_LTRS);
                figs = false;
            }
            out.push(e.0);
        } else if let Some(e) = C476.iter().find(|e| e.2 == c && c != 0x07) {
            if !figs {
                out.push(C476_FIGS);
                figs = true;
            }
            out.push(e.0);
        }
    }
    out
}

/// The longest run in `w` of a 2-4 character unit (A-Z, 0-9, at least
/// two different characters) repeated four or more times (RYRYRY...).
fn longest_repeat(w: &[char]) -> usize {
    let mut best = 0;
    for u in 2..=4 {
        for s in 0..w.len().saturating_sub(4 * u - 1) {
            let unit = &w[s..s + u];
            if !unit
                .iter()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                || unit.iter().all(|&c| c == unit[0])
            {
                continue;
            }
            let mut k = 1;
            while s + (k + 1) * u <= w.len() && &w[s + k * u..s + (k + 1) * u] == unit {
                k += 1;
            }
            if k >= 4 {
                best = best.max(k * u);
            }
        }
    }
    best
}

/// Plausibility of decoded teleprinter text, 0..1: the share of characters
/// in words of a normal length that read as words, numbers, calls or
/// separators (random codes give few spaces, so long "words").
pub fn text_score(text: &str) -> f64 {
    let t: String = text.chars().filter(|&c| c != '\r').collect();
    if t.trim().chars().count() < 4 {
        return 0.0;
    }
    let (mut good, mut bad) = (0.0, 0.0);
    for w in t.split([' ', '\n']).filter(|w| !w.is_empty()) {
        let up = w.to_uppercase();
        let n = up.chars().count() as f64;
        if n > 16.0 {
            let m = longest_repeat(&up.chars().collect::<Vec<_>>()) as f64;
            good += m;
            bad += n - m;
            continue;
        }
        let q = word_plaus(&up);
        good += n * q;
        bad += n * (1.0 - q);
    }
    good / (good + bad).max(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ita2_round_trip() {
        let codes = ita2_encode("CQ DE OH8ABC 599 73", true);
        let mut d = Ita2::new(true, false);
        let s: String = codes.iter().filter_map(|&c| d.char(c)).collect();
        assert_eq!(s, "CQ DE OH8ABC 599 73");
    }

    #[test]
    fn usos_decided_by_text() {
        // DWD style: figures kept across spaces (no USOS)
        let codes = ita2_encode("TEMP 12 15 18 1022 HPA 1018 1009 23 25", false);
        assert!(!ita2_usos(&codes, false));
        let codes = ita2_encode("CQ CQ DE OH8ABC 599 TNX FER CALL 73 ES GL", true);
        assert!(ita2_usos(&codes, false));
    }

    #[test]
    fn ccir476() {
        let mut d = Ccir476::default();
        let s: String = ccir476_encode("ZCZC EA12 GALE 8")
            .iter()
            .filter_map(|&c| d.char(c))
            .collect();
        assert_eq!(s, "ZCZC EA12 GALE 8");
        assert!(C476.iter().all(|e| w4(e.0)));
    }

    #[test]
    fn ascii() {
        assert_eq!(ascii_char(0x41, 7, false), Some('A'));
        assert_eq!(ascii_char(0b1000001, 7, true), Some('A')); // palindrome
        assert_eq!(ascii_char(0b1100001, 7, true), Some('C')); // 0x43 reversed
        assert_eq!(ascii_char(7, 7, false), None);
    }

    #[test]
    fn scores() {
        assert!(text_score("CQ CQ DE OH8ABC OH8ABC PSE K") > 0.9);
        assert!(text_score("GIBRALTAR OST 4 // 23 GRAD 1022 HPA") > 0.9);
        assert!(text_score("XQZTRWPLKJHGFDSXCVBNMQWRTZPS") < 0.2);
        assert!(text_score("RYRYRYRYRYRYRYRYRYRY") > 0.9);
        assert_eq!(word_plaus("QTH"), 1.0);
        assert_eq!(word_plaus("QAZ"), 1.0);
        assert_eq!(word_plaus("IRAQ"), 0.0);
        assert_eq!(word_plaus("15.10.26"), 1.0);
    }
}
