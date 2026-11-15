// SPDX-License-Identifier: GPL-3.0-or-later
//! A frame's 72 bits to text, as JS8Call writes it (`varicode.cpp`,
//! `decodedtext.cpp`). The 3 flag bits sent beside them say first frame
//! of a message (1), last (2), and fast data (4). The 72 bits start with
//! the frame type: 000 heartbeat (callsign, grid, CQ or HB), 001 compound
//! callsign, 010 compound callsign with a grid or command, 011 directed
//! (from, to, command, number), 1xx data (dictionary-compressed text, or
//! Huffman in old versions). Fast data frames (flag 4) are all text.

use crate::jsc;

/// First frame of a message, last, fast data.
pub const FIRST: u8 = 1;
pub const LAST: u8 = 2;
pub const DATA: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Heartbeat,
    Compound,
    CompoundDirected,
    Directed,
    Data,
}

/// A frame read.
#[derive(Clone, Debug, PartialEq)]
pub struct Unpacked {
    pub kind: Kind,
    /// As JS8Call shows it (a frame of a message: frames are joined).
    pub text: String,
    /// The sender and addressee where the frame says.
    pub from: Option<String>,
    pub to: Option<String>,
    pub grid: Option<String>,
}

/// The 72 bits, the first sent at bit 71.
struct Bits(u128);

impl Bits {
    fn get(&self, from: usize, n: usize) -> u64 {
        ((self.0 >> (72 - from - n)) & ((1u128 << n) - 1)) as u64
    }
}

const ALNUM: &[u8; 39] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ /@";
const NBASECALL: u32 = 37 * 36 * 10 * 27 * 27 * 27;
const NBASEGRID: u16 = 180 * 180;
const NUSERGRID: u16 = NBASEGRID + 10;
const NMAXGRID: u16 = (1 << 15) - 1;

/// Callsigns past `NBASECALL`: groups and the incomplete call.
const GROUPS: [&str; 54] = [
    "<....>",
    "@ALLCALL",
    "@JS8NET",
    "@DX/NA",
    "@DX/SA",
    "@DX/EU",
    "@DX/AS",
    "@DX/AF",
    "@DX/OC",
    "@DX/AN",
    "@REGION/1",
    "@REGION/2",
    "@REGION/3",
    "@GROUP/0",
    "@GROUP/1",
    "@GROUP/2",
    "@GROUP/3",
    "@GROUP/4",
    "@GROUP/5",
    "@GROUP/6",
    "@GROUP/7",
    "@GROUP/8",
    "@GROUP/9",
    "@COMMAND",
    "@CONTROL",
    "@NET",
    "@NTS",
    "@RESERVE/0",
    "@RESERVE/1",
    "@RESERVE/2",
    "@RESERVE/3",
    "@RESERVE/4",
    "@APRSIS",
    "@RAGCHEW",
    "@JS8",
    "@EMCOMM",
    "@ARES",
    "@MARS",
    "@AMRRON",
    "@RACES",
    "@RAYNET",
    "@RADAR",
    "@SKYWARN",
    "@CQ",
    "@HB",
    "@QSO",
    "@QSOPARTY",
    "@CONTEST",
    "@FIELDDAY",
    "@SOTA",
    "@IOTA",
    "@POTA",
    "@QRP",
    "@QRO",
];

/// Directed commands by number (where JS8Call has two names for one, the
/// one it shows: the first in its sorted map).
const CMDS: [&str; 32] = [
    " SNR?",
    " DIT DIT",
    " NACK",
    " HEARING?",
    " GRID?",
    ">",
    " STATUS?",
    " STATUS",
    " HEARING",
    " MSG",
    " MSG TO:",
    " QUERY",
    " QUERY MSGS",
    " QUERY CALL",
    " ACK",
    " GRID",
    " INFO?",
    " INFO",
    " FB",
    " HW CPY?",
    " SK",
    " RR",
    " QSL?",
    " QSL",
    " CMD",
    " SNR",
    " NO",
    " YES",
    " 73",
    " HEARTBEAT SNR",
    " AGN?",
    " ",
];

const CQS: [&str; 8] = [
    "CQ CQ CQ",
    "CQ DX",
    "CQ QRP",
    "CQ CONTEST",
    "CQ FIELD",
    "CQ FD",
    "CQ CQ",
    "CQ",
];

fn is_snr(cmd: &str) -> bool {
    cmd == " SNR" || cmd == " HEARTBEAT SNR"
}

pub fn format_snr(snr: i32) -> String {
    if !(-60..=60).contains(&snr) {
        return String::new();
    }
    if snr >= 0 {
        format!("+{snr:02}")
    } else {
        format!("-{:02}", -snr)
    }
}

fn alnum50(mut v: u64) -> String {
    let mut w = [b' '; 11];
    for (i, base) in [
        (10, 38),
        (9, 38),
        (8, 38),
        (7, 2),
        (6, 38),
        (5, 38),
        (4, 38),
        (3, 2),
        (2, 38),
        (1, 38),
        (0, 39),
    ] {
        let d = (v % base) as usize;
        v /= base;
        w[i] = if base == 2 {
            if d == 1 { b'/' } else { b' ' }
        } else {
            ALNUM[d]
        };
    }
    String::from_utf8_lossy(&w).replace(' ', "")
}

pub fn callsign(mut v: u32, portable: bool) -> String {
    if v > NBASECALL && v - NBASECALL <= GROUPS.len() as u32 {
        return GROUPS[(v - NBASECALL - 1) as usize].into();
    }
    let mut w = [b' '; 6];
    for i in [5, 4, 3] {
        w[i] = ALNUM[(v % 27 + 10) as usize];
        v /= 27;
    }
    w[2] = ALNUM[(v % 10) as usize];
    v /= 10;
    w[1] = ALNUM[(v % 36) as usize];
    v /= 36;
    w[0] = ALNUM[(v as usize).min(38)];
    let mut c = String::from_utf8_lossy(&w).to_string();
    if let Some(r) = c.strip_prefix("3D0") {
        c = format!("3DA0{r}");
    }
    if c.starts_with('Q') && c.as_bytes().get(1).is_some_and(|b| b.is_ascii_uppercase()) {
        c = format!("3X{}", &c[1..]);
    }
    let c = c.trim().to_string();
    if portable { format!("{c}/P") } else { c }
}

/// A 4-character locator, or "" (the value says none).
pub fn grid(v: u16) -> String {
    if v > NBASEGRID {
        return String::new();
    }
    let dlat = (v % 180) as f64 - 90.0;
    let dlong = (v / 180) as f64 * 2.0 - 180.0 + 2.0;
    let nlong = (60.0 * (180.0 - dlong) / 5.0) as i32;
    let nlat = (60.0 * (dlat + 90.0) / 2.5) as i32;
    let c = |n: i32, base: u8| (base + n as u8) as char;
    format!(
        "{}{}{}{}",
        c(nlong / 240, b'A'),
        c(nlat / 240, b'A'),
        c((nlong % 240) / 24, b'0'),
        c((nlat % 240) / 24, b'0')
    )
}

fn unpack_cmd(v: u16) -> (Option<&'static str>, u8) {
    if v & (1 << 7) != 0 {
        let cmd = if v & (1 << 6) != 0 {
            CMDS[29]
        } else {
            CMDS[25]
        };
        (Some(cmd), (v & 63) as u8)
    } else {
        (CMDS.get((v & 127) as usize).copied(), 0)
    }
}

/// Unpacks a frame: its 72 bits (the first at bit 71) and its 3 flag bits.
pub fn unpack(payload: u128, flags: u8) -> Option<Unpacked> {
    let b = Bits(payload);
    if flags & DATA != 0 {
        // all 72 bits text, ones padded after a zero
        let n = (0..72).rev().find(|&i| b.get(i, 1) == 0)?;
        let bits: Vec<bool> = (0..n).map(|i| b.get(i, 1) == 1).collect();
        return data(jsc::decompress(&bits)?);
    }
    let kind = b.get(0, 3) as u8;
    if kind & 4 != 0 {
        // legacy data: a compressed flag, then the text, then the padding
        let compressed = b.get(1, 1) == 1;
        let n = (2..72).rev().find(|&i| b.get(i, 1) == 0)?;
        let bits: Vec<bool> = (2..n).map(|i| b.get(i, 1) == 1).collect();
        let t = if compressed {
            jsc::decompress(&bits)?
        } else {
            huffman(&bits)
        };
        return data(t);
    }
    let rem = b.get(64, 8) as u16;
    match kind {
        0..=2 => {
            let call = alnum50(b.get(3, 50));
            let num = ((b.get(53, 11) as u16) << 5) | (rem >> 3);
            let bits3 = (rem & 7) as usize;
            match kind {
                0 => {
                    let g = grid(num & 0x7fff);
                    let what = if num & 0x8000 != 0 {
                        format!("@ALLCALL {}", CQS[bits3])
                    } else {
                        "@HB HEARTBEAT".into()
                    };
                    Some(Unpacked {
                        kind: Kind::Heartbeat,
                        text: format!("{call}: {what} {g} "),
                        from: Some(call),
                        to: None,
                        grid: (!g.is_empty()).then_some(g),
                    })
                }
                1 => Some(Unpacked {
                    kind: Kind::Compound,
                    text: format!("{call}: "),
                    from: Some(call),
                    to: None,
                    grid: None,
                }),
                _ => {
                    let mut extra = String::new();
                    let mut g = None;
                    if num <= NBASEGRID {
                        let s = grid(num);
                        extra = format!(" {s}");
                        g = Some(s);
                    } else if (NUSERGRID..NMAXGRID).contains(&num) {
                        let (cmd, n) = unpack_cmd(num - NUSERGRID);
                        let cmd = cmd.unwrap_or("");
                        extra = cmd.to_string();
                        if is_snr(cmd) {
                            extra += " ";
                            extra += &format_snr(n as i32 - 31);
                        }
                    }
                    Some(Unpacked {
                        kind: Kind::CompoundDirected,
                        text: format!("{call}{extra} "),
                        from: Some(call),
                        to: None,
                        grid: g,
                    })
                }
            }
        }
        _ => {
            // directed
            let from = callsign(b.get(3, 28) as u32, rem >> 7 & 1 == 1);
            let to = callsign(b.get(31, 28) as u32, rem >> 6 & 1 == 1);
            let cmd = CMDS[(b.get(59, 5) % 32) as usize];
            let extra = rem % 64;
            let mut text = format!("{from}: {to}{cmd}");
            if extra != 0 {
                text += " ";
                text += &if is_snr(cmd) {
                    format_snr(extra as i32 - 31)
                } else {
                    (extra as i32 - 31).to_string()
                };
            }
            text += " ";
            Some(Unpacked {
                kind: Kind::Directed,
                text,
                from: Some(from),
                to: Some(to),
                grid: None,
            })
        }
    }
}

fn data(text: String) -> Option<Unpacked> {
    (!text.is_empty()).then_some(Unpacked {
        kind: Kind::Data,
        text: unescape(&text),
        from: None,
        to: None,
        grid: None,
    })
}

/// JS8Call's escapes for characters outside ASCII: `\Uxxxx` or `U+xxxx`.
fn unescape(t: &str) -> String {
    let mut out = String::new();
    let mut rest = t;
    while let Some(i) = rest.find(['\\', 'U', 'u']) {
        let (head, tail) = rest.split_at(i);
        out += head;
        let prefix = if tail.len() >= 2
            && (tail.starts_with("\\U")
                || tail.starts_with("\\u")
                || tail.starts_with("U+")
                || tail.starts_with("u+"))
        {
            2
        } else {
            0
        };
        let hex = tail.get(prefix..prefix + 4);
        match hex
            .filter(|_| prefix > 0)
            .and_then(|h| u32::from_str_radix(h, 16).ok())
            .and_then(char::from_u32)
        {
            Some(c) => {
                out.push(c);
                rest = &tail[prefix + 4..];
            }
            None => {
                let c = tail.chars().next().unwrap();
                out.push(c);
                rest = &tail[c.len_utf8()..];
            }
        }
    }
    out + rest
}

/// JS8Call's original Huffman table (data frames before 2.2).
const HUFF: [(&str, &str); 44] = [
    (" ", "01"),
    ("E", "100"),
    ("T", "1101"),
    ("A", "0011"),
    ("O", "11111"),
    ("I", "11100"),
    ("N", "10111"),
    ("S", "10100"),
    ("H", "00011"),
    ("R", "00000"),
    ("D", "111011"),
    ("L", "110011"),
    ("C", "110001"),
    ("U", "101101"),
    ("M", "101011"),
    ("W", "001011"),
    ("F", "001001"),
    ("G", "000101"),
    ("Y", "000011"),
    ("P", "1111011"),
    ("B", "1111001"),
    (".", "1110100"),
    ("V", "1100101"),
    ("K", "1100100"),
    ("-", "1100001"),
    ("+", "1100000"),
    ("?", "1011001"),
    ("!", "1011000"),
    ("\"", "1010101"),
    ("X", "1010100"),
    ("0", "0010101"),
    ("J", "0010100"),
    ("1", "0010001"),
    ("Q", "0010000"),
    ("2", "0001001"),
    ("Z", "0001000"),
    ("3", "0000101"),
    ("5", "0000100"),
    ("4", "11110101"),
    ("9", "11110100"),
    ("8", "11110001"),
    ("6", "11110000"),
    ("7", "11101011"),
    ("/", "11101010"),
];

fn huffman(bits: &[bool]) -> String {
    let s: String = bits.iter().map(|&b| if b { '1' } else { '0' }).collect();
    let mut rest = s.as_str();
    let mut out = String::new();
    'next: while !rest.is_empty() {
        for (ch, code) in HUFF {
            if let Some(r) = rest.strip_prefix(code) {
                out += ch;
                rest = r;
                continue 'next;
            }
        }
        break;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snr_and_grid() {
        assert_eq!(format_snr(-5), "-05");
        assert_eq!(format_snr(7), "+07");
        assert_eq!(format_snr(-24), "-24");
        // FN42 as JS8Call packs it: longitude west-positive, 70 (from
        // 70.96 W), latitude 42 + 90
        let v = ((70 + 180) / 2) * 180 + (42 + 90);
        assert_eq!(grid(v), "FN42");
        assert_eq!(grid(NMAXGRID), "");
    }

    #[test]
    fn callsigns() {
        // N0CALL is 6 characters: N 0 C A L L doesn't fit (digit third);
        // " N0ABC": space, N, 0, A, B, C
        let v = (((36 * 36 + 23) * 10 * 27) * 27 + 1) * 27 + 2;
        assert_eq!(callsign(v, false), "N0ABC");
        assert_eq!(callsign(v, true), "N0ABC/P");
        assert_eq!(callsign(NBASECALL + 2, false), "@ALLCALL");
        assert_eq!(callsign(NBASECALL + 45, false), "@HB");
    }

    #[test]
    fn unescapes() {
        assert_eq!(unescape("CAF\\U00c9 OK"), "CAFÉ OK");
        assert_eq!(unescape("USA U+00e9"), "USA é");
        assert_eq!(huffman(&[false, true, true, false, false]), " E");
    }
}
