// SPDX-License-Identifier: GPL-3.0-or-later
//! The 77-bit FT8/FT4 message: unpacking to text (every type seen on HF)
//! and packing the common ones, with the table of hashed callsigns.
//!
//! The word is a `u128` whose bit 76 is the first bit sent. The type is
//! `i3` (the last 3 bits) and, for `i3 = 0`, `n3` (the 3 before them).

use std::collections::HashMap;

const NTOKENS: u32 = 2_063_592;
const MAX22: u32 = 4_194_304;
const MAXGRID4: u32 = 32_400;

const A37: &[u8] = b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const A36: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const A27: &[u8] = b" ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const A38: &[u8] = b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ/";
const A42: &[u8] = b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ+-./?";

/// ARRL/RAC sections (Field Day, type 0.3/0.4).
const SECTIONS: [&str; 86] = [
    "AB", "AK", "AL", "AR", "AZ", "BC", "CO", "CT", "DE", "EB", "EMA", "ENY", "EPA", "EWA", "GA",
    "GH", "IA", "ID", "IL", "IN", "KS", "KY", "LA", "LAX", "NS", "MB", "MDC", "ME", "MI", "MN",
    "MO", "MS", "MT", "NC", "ND", "NE", "NFL", "NH", "NL", "NLI", "NM", "NNJ", "NNY", "TER", "NTX",
    "NV", "OH", "OK", "ONE", "ONN", "ONS", "OR", "ORG", "PAC", "PR", "QC", "RI", "SB", "SC", "SCV",
    "SD", "SDG", "SF", "SFL", "SJV", "SK", "SNJ", "STX", "SV", "TN", "UT", "VA", "VI", "VT", "WCF",
    "WI", "WMA", "WNY", "WPA", "WTX", "WV", "WWA", "WY", "DX", "PE", "NB",
];

/// US states, Canadian provinces and DX multipliers (RTTY Roundup, type 3).
const MULTS: [&str; 72] = [
    "AL", "AK", "AZ", "AR", "CA", "CO", "CT", "DE", "FL", "GA", "HI", "ID", "IL", "IN", "IA", "KS",
    "KY", "LA", "ME", "MD", "MA", "MI", "MN", "MS", "MO", "MT", "NE", "NV", "NH", "NJ", "NM", "NY",
    "NC", "ND", "OH", "OK", "OR", "PA", "RI", "SC", "SD", "TN", "TX", "UT", "VT", "VA", "WA", "WV",
    "WI", "WY", "NB", "NS", "QC", "ON", "MB", "SK", "AB", "BC", "NWT", "NF", "LB", "NU", "YT",
    "PEI", "DC", "DR", "FR", "GD", "GR", "OV", "ZH", "ZL",
];

fn mult(i: u32) -> Option<String> {
    // 1-based; after the named ones come X01..X99.
    let i = i as usize;
    match i {
        1..=72 => Some(MULTS[i - 1].to_string()),
        73..=171 => Some(format!("X{:02}", i - 72)),
        _ => None,
    }
}

fn idx(alpha: &[u8], c: u8) -> Option<u32> {
    alpha.iter().position(|&a| a == c).map(|p| p as u32)
}

/// Bits `[start, start + n)` of the message, counted from the first bit sent.
fn bits(m: u128, start: u32, n: u32) -> u128 {
    (m >> (77 - start - n)) & ((1u128 << n) - 1)
}

/// Hash kinds: the 22-bit hash and its top 12 and 10 bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HashBits {
    H10,
    H12,
    H22,
}

/// The 22-bit hash of a callsign (up to 11 characters of `A38`).
pub fn hash22(call: &str) -> Option<u32> {
    let b = call.as_bytes();
    if b.len() > 11 {
        return None;
    }
    let mut n: u64 = 0;
    for i in 0..11 {
        let c = b.get(i).copied().unwrap_or(b' ');
        n = n * 38 + idx(A38, c)? as u64;
    }
    Some((47_055_833_459u64.wrapping_mul(n) >> (64 - 22)) as u32 & 0x3f_ffff)
}

/// Callsigns heard, by hash, so `<...>` in later messages can be filled
/// in. Bounded: it starts over when it gets large.
#[derive(Default)]
pub struct CallHashes {
    by22: HashMap<u32, String>,
}

impl CallHashes {
    const MAX: usize = 20_000;

    pub fn save(&mut self, call: &str) {
        let call = call.trim_matches(['<', '>']);
        if call.len() < 3 || call.starts_with("CQ") {
            return;
        }
        if let Some(h) = hash22(call) {
            if self.by22.len() >= Self::MAX {
                self.by22.clear();
            }
            self.by22.insert(h, call.to_string());
        }
    }

    /// Whether a call (without brackets) has been heard.
    pub fn knows(&self, call: &str) -> bool {
        hash22(call).is_some_and(|h| self.by22.get(&h).is_some_and(|c| c == call))
    }

    pub fn lookup(&self, kind: HashBits, h: u32) -> Option<&str> {
        match kind {
            HashBits::H22 => self.by22.get(&h).map(|s| s.as_str()),
            HashBits::H12 => self
                .by22
                .iter()
                .find(|(k, _)| *k >> 10 == h)
                .map(|(_, v)| v.as_str()),
            HashBits::H10 => self
                .by22
                .iter()
                .find(|(k, _)| *k >> 12 == h)
                .map(|(_, v)| v.as_str()),
        }
    }

    fn bracket(&self, kind: HashBits, h: u32) -> String {
        match self.lookup(kind, h) {
            Some(c) => format!("<{c}>"),
            None => "<...>".to_string(),
        }
    }
}

/// A decoded message: its text and the parts the spot reporting needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub text: String,
    /// The transmitting station, when the type says who it is.
    pub sender: Option<String>,
    /// A 4-character locator sent with it.
    pub grid: Option<String>,
    /// The callsigns in it, in full (not tokens or hashes).
    pub calls: Vec<String>,
    /// The type: `i3`, and `n3` when `i3` is 0.
    pub i3: u8,
    pub n3: u8,
}

/// A standard callsign or token from its 28-bit code. `None` if invalid.
fn unpack28(n28: u32, hashes: &CallHashes) -> Option<String> {
    if n28 < NTOKENS {
        return match n28 {
            0 => Some("DE".into()),
            1 => Some("QRZ".into()),
            2 => Some("CQ".into()),
            3..=1002 => Some(format!("CQ {:03}", n28 - 3)),
            1003..=532_443 => {
                let mut n = n28 - 1003;
                let mut s = [b' '; 4];
                for i in (0..4).rev() {
                    s[i] = A27[(n % 27) as usize];
                    n /= 27;
                }
                Some(format!("CQ {}", std::str::from_utf8(&s).ok()?.trim_start()))
            }
            _ => None,
        };
    }
    let n = n28 - NTOKENS;
    if n < MAX22 {
        return Some(hashes.bracket(HashBits::H22, n));
    }
    let mut n = n - MAX22;
    let mut c = [b' '; 6];
    c[5] = A27[(n % 27) as usize];
    n /= 27;
    c[4] = A27[(n % 27) as usize];
    n /= 27;
    c[3] = A27[(n % 27) as usize];
    n /= 27;
    c[2] = b'0' + (n % 10) as u8;
    n /= 10;
    c[1] = A36[(n % 36) as usize];
    n /= 36;
    c[0] = *A37.get(n as usize)?;
    let s = std::str::from_utf8(&c).ok()?;
    let call = if let Some(rest) = s.strip_prefix("3D0").filter(|r| !r.starts_with(' ')) {
        // Eswatini: 3DA0XYZ is sent as 3D0XYZ.
        format!("3DA0{}", rest.trim())
    } else if c[0] == b'Q' && c[1].is_ascii_uppercase() {
        // Guinea: 3XA0XYZ is sent as QA0XYZ.
        format!("3X{}", s[1..].trim())
    } else {
        s.trim().to_string()
    };
    // A real call has no spaces inside and is at least 3 characters.
    if call.len() < 3 || call.contains(' ') {
        return None;
    }
    Some(call)
}

fn grid4(n: u32) -> String {
    let c = [
        b'A' + (n / 1800) as u8,
        b'A' + (n / 100 % 18) as u8,
        b'0' + (n / 10 % 10) as u8,
        b'0' + (n % 10) as u8,
    ];
    String::from_utf8_lossy(&c).into_owned()
}

fn report(r: i32) -> String {
    format!("{}{:02}", if r < 0 { '-' } else { '+' }, r.abs())
}

/// A callsign that looks like one: an ITU-style prefix (letter-letter,
/// letter-digit or digit-letter, not starting with 0, 1 or Q), a digit, and
/// 1-3 letters, with an optional /P or /R. Hashed (`<...>`) calls pass.
pub fn plausible_call(c: &str) -> bool {
    if c.starts_with('<') {
        return true;
    }
    let c = c
        .strip_suffix("/P")
        .or_else(|| c.strip_suffix("/R"))
        .unwrap_or(c);
    let b = c.as_bytes();
    let up = |x: u8| x.is_ascii_uppercase();
    let dig = |x: u8| x.is_ascii_digit();
    // Prefix of 1 or 2 characters, then the area digit, then a suffix.
    let prefix_ok = |p: &[u8]| match p {
        [a] => up(*a) && *a != b'Q',
        [a, b] => {
            (up(*a) && *a != b'Q' && (up(*b) || dig(*b))) || (dig(*a) && *a >= b'2' && up(*b))
        }
        _ => false,
    };
    for plen in [1, 2] {
        if b.len() > plen + 1 && prefix_ok(&b[..plen]) && dig(b[plen]) {
            let suffix = &b[plen + 1..];
            if (1..=3).contains(&suffix.len()) && suffix.iter().all(|&x| up(x)) {
                return true;
            }
        }
    }
    // 3DA0XYZ and 3XA0XYZ style.
    if b.len() >= 5 && (c.starts_with("3DA") || c.starts_with("3X")) {
        return true;
    }
    false
}

fn is_call(s: &str) -> bool {
    !s.starts_with('<') && !s.starts_with("CQ") && s != "DE" && s != "QRZ"
}

/// Unpack a message; `None` if it isn't a valid one. Callsigns seen are
/// saved in `hashes`.
pub fn unpack(m: u128, hashes: &mut CallHashes) -> Option<Message> {
    let msg = peek(m, hashes)?;
    for c in &msg.calls {
        hashes.save(c);
    }
    Some(msg)
}

/// Unpack without saving its callsigns (to check a doubtful decode first).
pub fn peek(m: u128, hashes: &CallHashes) -> Option<Message> {
    let mut calls: Vec<String> = vec![];
    let i3 = (m & 7) as u32;
    let n3 = (m >> 3 & 7) as u32;
    let mut sender = None;
    let mut grid = None;
    let text = match (i3, n3) {
        (0, 0) => {
            // Free text: 13 characters, base 42.
            let mut n = bits(m, 0, 71);
            let mut c = [b' '; 13];
            for i in (0..13).rev() {
                c[i] = A42[(n % 42) as usize];
                n /= 42;
            }
            std::str::from_utf8(&c).ok()?.trim().to_string()
        }
        (0, 1) => {
            // DXpedition: K1ABC RR73; W9XYZ <KH1/KH7Z> -12
            let c1 = unpack28(bits(m, 0, 28) as u32, hashes)?;
            let c2 = unpack28(bits(m, 28, 28) as u32, hashes)?;
            let h10 = bits(m, 56, 10) as u32;
            let r = bits(m, 66, 5) as i32 * 2 - 30;
            let c3 = hashes.bracket(HashBits::H10, h10);
            for c in [&c1, &c2] {
                if is_call(c) {
                    calls.push(c.to_string());
                }
            }
            sender = Some(c3.clone()).filter(|c| is_call(c));
            format!("{c1} RR73; {c2} {c3} {}", report(r))
        }
        (0, 3) | (0, 4) => {
            // ARRL Field Day: WA9XYZ KA1ABC R 16A EMA
            let c1 = unpack28(bits(m, 0, 28) as u32, hashes)?;
            let c2 = unpack28(bits(m, 28, 28) as u32, hashes)?;
            let ir = bits(m, 56, 1) != 0;
            let ntx = bits(m, 57, 4) as u32 + 1 + if n3 == 4 { 16 } else { 0 };
            let class = (b'A' + bits(m, 61, 3) as u8) as char;
            let sec = SECTIONS.get((bits(m, 64, 7) as usize).checked_sub(1)?)?;
            for c in [&c1, &c2] {
                if is_call(c) {
                    calls.push(c.to_string());
                }
            }
            sender = Some(c2.clone()).filter(|c| is_call(c));
            format!("{c1} {c2} {}{ntx}{class} {sec}", if ir { "R " } else { "" })
        }
        (0, 5) => {
            // Telemetry: 71 bits as 18 hex digits, leading zeros dropped.
            let s = format!("{:018X}", bits(m, 0, 71));
            let t = s.trim_start_matches('0');
            if t.is_empty() {
                "0".to_string()
            } else {
                t.to_string()
            }
        }
        (1, _) | (2, _) => {
            // Standard: WA9XYZ/R KA1ABC/R R FN42
            let suffix = if i3 == 1 { "/R" } else { "/P" };
            let mut c1 = unpack28(bits(m, 0, 28) as u32, hashes)?;
            let pa = bits(m, 28, 1) != 0;
            let mut c2 = unpack28(bits(m, 29, 28) as u32, hashes)?;
            let pb = bits(m, 57, 1) != 0;
            let ir = bits(m, 58, 1) != 0;
            let g = bits(m, 59, 15) as u32;
            if pa {
                if !is_call(&c1) {
                    return None;
                }
                c1.push_str(suffix);
            }
            if pb {
                if !is_call(&c2) {
                    return None;
                }
                c2.push_str(suffix);
            }
            // The first word can't be a hashed call next to CQ etc. in
            // the second slot.
            if !is_call(&c2) && !c2.starts_with('<') {
                return None;
            }
            for c in [&c1, &c2] {
                if is_call(c) {
                    calls.push(c.to_string());
                }
            }
            sender = Some(c2.clone()).filter(|c| is_call(c));
            let extra = if g < MAXGRID4 {
                let g4 = grid4(g);
                grid = Some(g4.clone());
                if ir {
                    format!(" R {g4}")
                } else {
                    format!(" {g4}")
                }
            } else {
                match g - MAXGRID4 {
                    1 => String::new(),
                    2 => " RRR".into(),
                    3 => " RR73".into(),
                    4 => " 73".into(),
                    r => {
                        let r = report(r as i32 - 35);
                        if ir {
                            format!(" R{r}")
                        } else {
                            format!(" {r}")
                        }
                    }
                }
            };
            // A report or roger after CQ isn't a real message.
            if c1.starts_with("CQ") && (ir || g >= MAXGRID4 + 2) {
                return None;
            }
            format!("{c1} {c2}{extra}")
        }
        (3, _) => {
            // ARRL RTTY Roundup: TU; W9XYZ K1ABC R 579 MA
            let tu = bits(m, 0, 1) != 0;
            let c1 = unpack28(bits(m, 1, 28) as u32, hashes)?;
            let c2 = unpack28(bits(m, 29, 28) as u32, hashes)?;
            let ir = bits(m, 57, 1) != 0;
            let rst = format!("5{}9", bits(m, 58, 3) as u32 + 2);
            let x = bits(m, 61, 13) as u32;
            let exch = if x > 8000 {
                mult(x - 8000)?
            } else if (1..=7999).contains(&x) {
                format!("{x:04}")
            } else {
                return None;
            };
            for c in [&c1, &c2] {
                if is_call(c) {
                    calls.push(c.to_string());
                }
            }
            sender = Some(c2.clone()).filter(|c| is_call(c));
            format!(
                "{}{c1} {c2} {}{rst} {exch}",
                if tu { "TU; " } else { "" },
                if ir { "R " } else { "" }
            )
        }
        (4, _) => {
            // One nonstandard call and one hashed: <WA9XYZ> PJ4/KA1ABC RR73
            let h12 = bits(m, 0, 12) as u32;
            let mut n58 = bits(m, 12, 58) as u64;
            let flip = bits(m, 70, 1) != 0;
            let r = bits(m, 71, 2) as u32;
            let cq = bits(m, 73, 1) != 0;
            let mut c = [b' '; 11];
            for i in (0..11).rev() {
                c[i] = A38[(n58 % 38) as usize];
                n58 /= 38;
            }
            let call = std::str::from_utf8(&c).ok()?.trim().to_string();
            if call.len() < 3 || call.contains(' ') {
                return None;
            }
            calls.push(call.clone());
            sender = Some(call.clone());
            if cq {
                format!("CQ {call}")
            } else {
                let h = hashes.bracket(HashBits::H12, h12);
                let (c1, c2) = if flip { (call, h) } else { (h, call) };
                if flip {
                    sender = Some(c2.clone()).filter(|c| is_call(c));
                }
                let rpt = ["", " RRR", " RR73", " 73"][r as usize];
                format!("{c1} {c2}{rpt}")
            }
        }
        (5, _) => {
            // EU VHF: <PA3XYZ> <G4ABC/P> R 590003 IO91NP
            let h12 = bits(m, 0, 12) as u32;
            let h22 = bits(m, 12, 22) as u32;
            let ir = bits(m, 34, 1) != 0;
            let r = bits(m, 35, 3) as u32 + 2;
            let serial = bits(m, 38, 11) as u32;
            let g6 = bits(m, 49, 25) as u32;
            let c1 = hashes.bracket(HashBits::H12, h12);
            let c2 = hashes.bracket(HashBits::H22, h22);
            let n = g6;
            let loc = [
                b'A' + (n / (18 * 10 * 10 * 24 * 24)) as u8,
                b'A' + (n / (10 * 10 * 24 * 24) % 18) as u8,
                b'0' + (n / (10 * 24 * 24) % 10) as u8,
                b'0' + (n / (24 * 24) % 10) as u8,
                b'A' + (n / 24 % 24) as u8,
                b'A' + (n % 24) as u8,
            ];
            if loc[0] > b'R' {
                return None;
            }
            format!(
                "{c1} {c2} {}5{r}{serial:04} {}",
                if ir { "R " } else { "" },
                String::from_utf8_lossy(&loc)
            )
        }
        _ => return None,
    };
    Some(Message {
        text,
        sender,
        grid,
        calls,
        i3: i3 as u8,
        n3: if i3 == 0 { n3 as u8 } else { 0 },
    })
}

/// Pack a standard callsign (or token) into 28 bits, with the flag for
/// a /R or /P suffix.
fn pack28(word: &str) -> Option<(u32, bool)> {
    match word {
        "DE" => return Some((0, false)),
        "QRZ" => return Some((1, false)),
        "CQ" => return Some((2, false)),
        _ => {}
    }
    if let Some(rest) = word.strip_prefix("CQ ") {
        if rest.len() == 3 && rest.bytes().all(|c| c.is_ascii_digit()) {
            return Some((3 + rest.parse::<u32>().ok()?, false));
        }
        if (1..=4).contains(&rest.len()) && rest.bytes().all(|c| c.is_ascii_uppercase()) {
            let mut n = 0;
            for i in 0..4 {
                let c = if i < 4 - rest.len() {
                    b' '
                } else {
                    rest.as_bytes()[i - (4 - rest.len())]
                };
                n = n * 27 + idx(A27, c)?;
            }
            return Some((1003 + n, false));
        }
        return None;
    }
    if let Some(inner) = word.strip_prefix('<').and_then(|w| w.strip_suffix('>')) {
        return Some((NTOKENS + hash22(inner)?, false));
    }
    let (base, p) = match word.strip_suffix("/R").or_else(|| word.strip_suffix("/P")) {
        Some(b) => (b, true),
        None => (word, false),
    };
    let base = if let Some(r) = base.strip_prefix("3DA0") {
        format!("3D0{r}")
    } else if base.starts_with("3X")
        && base
            .as_bytes()
            .get(2)
            .is_some_and(|c| c.is_ascii_uppercase())
    {
        format!("Q{}", &base[2..])
    } else {
        base.to_string()
    };
    let b = base.as_bytes();
    let mut c = [b' '; 6];
    if b.len() >= 3 && b[2].is_ascii_digit() && b.len() <= 6 {
        c[..b.len()].copy_from_slice(b);
    } else if b.len() >= 2 && b[1].is_ascii_digit() && b.len() <= 5 {
        c[1..=b.len()].copy_from_slice(b);
    } else {
        return None;
    }
    let mut n = idx(A37, c[0])?;
    n = n * 36 + idx(A36, c[1])?;
    n = n * 10 + idx(b"0123456789", c[2])?;
    for &x in &c[3..] {
        n = n * 27 + idx(A27, x)?;
    }
    Some((NTOKENS + MAX22 + n, p))
}

fn pack_grid_or_report(w: Option<&str>) -> Option<(u32, bool)> {
    let Some(w) = w else {
        return Some((MAXGRID4 + 1, false));
    };
    match w {
        "RRR" => return Some((MAXGRID4 + 2, false)),
        "RR73" => return Some((MAXGRID4 + 3, false)),
        "73" => return Some((MAXGRID4 + 4, false)),
        _ => {}
    }
    let b = w.as_bytes();
    if b.len() == 4
        && (b'A'..=b'R').contains(&b[0])
        && (b'A'..=b'R').contains(&b[1])
        && b[2].is_ascii_digit()
        && b[3].is_ascii_digit()
    {
        let n = (b[0] - b'A') as u32 * 1800
            + (b[1] - b'A') as u32 * 100
            + (b[2] - b'0') as u32 * 10
            + (b[3] - b'0') as u32;
        return Some((n, false));
    }
    let (ir, r) = match w.strip_prefix('R') {
        Some(r) => (true, r),
        None => (false, w),
    };
    if !(r.starts_with('+') || r.starts_with('-')) {
        return None;
    }
    let v: i32 = r.parse().ok()?;
    let v = v.clamp(-30, 99);
    Some((MAXGRID4 + (v + 35) as u32, ir))
}

/// Pack a message of the common kinds: standard (types 1/2), free text
/// (0.0). Enough for tests and transmit-side tools.
pub fn pack(text: &str) -> Option<u128> {
    let text = text.trim().to_ascii_uppercase();
    let mut words: Vec<String> = text.split_whitespace().map(str::to_string).collect();
    // "CQ DX K1ABC FN42": the CQ modifier is part of the first word.
    if words.len() >= 3
        && words[0] == "CQ"
        && pack28(&format!("CQ {}", words[1])).is_some()
        && pack28(&words[2]).is_some_and(|(n, _)| n >= NTOKENS + MAX22)
    {
        let w = format!("CQ {}", words.remove(1));
        words[0] = w;
    }
    let std = (|| {
        if !(2..=4).contains(&words.len()) {
            return None;
        }
        let (n1, p1) = pack28(&words[0])?;
        let (n2, p2) = pack28(&words[1])?;
        let (extra, mut ir) = match words.len() {
            2 => (None, false),
            3 => (Some(words[2].as_str()), false),
            _ if words[2] == "R" => (Some(words[3].as_str()), true),
            _ => return None,
        };
        let (g, r2) = pack_grid_or_report(extra)?;
        ir |= r2;
        let slash_p = words[0].ends_with("/P") || words[1].ends_with("/P");
        let i3: u128 = if slash_p { 2 } else { 1 };
        Some(
            (n1 as u128) << 49
                | (p1 as u128) << 48
                | (n2 as u128) << 20
                | (p2 as u128) << 19
                | (ir as u128) << 18
                | (g as u128) << 3
                | i3,
        )
    })();
    if std.is_some() {
        return std;
    }
    // Free text, up to 13 characters.
    if text.len() > 13 {
        return None;
    }
    let mut n: u128 = 0;
    for i in 0..13 {
        let c = text.as_bytes().get(i).copied().unwrap_or(b' ');
        n = n * 42 + idx(A42, c)? as u128;
    }
    Some(n << 6)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits_of(s: &str) -> u128 {
        u128::from_str_radix(s, 2).unwrap()
    }

    #[test]
    fn ft8code_example() {
        let m = bits_of(
            "00000000000000000000000000100000010011011110111100011010100010100001100110001",
        );
        let mut h = CallHashes::default();
        let msg = unpack(m, &mut h).unwrap();
        assert_eq!(msg.text, "CQ K1ABC FN42");
        assert_eq!(msg.sender.as_deref(), Some("K1ABC"));
        assert_eq!(msg.grid.as_deref(), Some("FN42"));
        assert_eq!(pack("CQ K1ABC FN42"), Some(m));
    }

    #[test]
    fn round_trips() {
        let mut h = CallHashes::default();
        for t in [
            "CQ K1ABC FN42",
            "K1ABC W9XYZ -07",
            "K1ABC W9XYZ R-12",
            "K1ABC W9XYZ RR73",
            "K1ABC W9XYZ 73",
            "K1ABC W9XYZ RRR",
            "K1ABC W9XYZ R FN42",
            "K1ABC W9XYZ",
            "CQ DX OH1ABC KP20",
            "CQ 123 K1ABC FN42",
            "PA3XYZ/P GM4ABC/P R JO22",
            "W9XYZ/R K1ABC +03",
            "3DA0XYZ K1ABC -10",
            "TNX BOB 73 GL",
        ] {
            let m = pack(t).unwrap_or_else(|| panic!("pack {t}"));
            assert_eq!(unpack(m, &mut h).map(|m| m.text).as_deref(), Some(t), "{t}");
        }
    }

    #[test]
    fn hashed_calls_resolve_once_heard() {
        let mut h = CallHashes::default();
        let m = pack("K1ABC <PJ4/KA1ABC> -10").unwrap();
        assert_eq!(unpack(m, &mut h).unwrap().text, "K1ABC <...> -10");
        h.save("PJ4/KA1ABC");
        assert_eq!(unpack(m, &mut h).unwrap().text, "K1ABC <PJ4/KA1ABC> -10");
    }
}

#[cfg(test)]
mod ft8code {
    use super::*;

    /// Every message type in `ft8code -t` unpacks to WSJT-X's text, once
    /// the hashed calls are known.
    #[test]
    fn unpacks_like_wsjtx() {
        let data = include_str!("../tests/data/ft8code.txt");
        let mut bad = vec![];
        for line in data.lines().filter(|l| !l.starts_with('#')) {
            let (b, want) = line.split_once('\t').unwrap();
            let m = u128::from_str_radix(b, 2).unwrap();
            let mut h = CallHashes::default();
            for w in want.split([' ', ';']) {
                if w.starts_with('<') {
                    h.save(w);
                }
            }
            let got = unpack(m, &mut h).map(|m| m.text);
            if got.as_deref() != Some(want) {
                bad.push(format!("{want:?} -> {got:?}"));
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }
}
