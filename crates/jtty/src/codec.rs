// SPDX-License-Identifier: GPL-3.0-or-later
//! Message grammar: each frame carries one 32-bit word, a TEXT5 literal
//! (five characters) or a structured atom (a call with an action, an
//! exchange number or location, a zone/class pair, serial and time, a
//! control phrase, a grid), plus an end-of-message bit (jtty-cuf
//! `message_codec.cpp`). Text is ASCII.

use crate::protocol::{
    ALPHABET, EncodedFrame, PayloadBits, alphabet_index, encode_payload, payload_from_word,
    word_from_payload,
};

pub const MAX_MESSAGE_BYTES: usize = 80;
pub const MAX_MESSAGE_FRAMES: usize = 16;

const BASE36: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const CALL_FIRST: &[u8] = b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const CALL_SECOND: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const CALL_DIGIT: &[u8] = b"0123456789";
const CALL_SUFFIX: &[u8] = b" ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const CALL_OFFSET: u32 = 6_257_896;
const CONTROLS: [&str; 18] = [
    "AGN?", "CALL?", "AGN CALL", "NR?", "AGN NR", "EXCH?", "STATE?", "SECTION?", "ZONE?", "GRID?",
    "RPRT?", "QSL TU", "TU", "QRZ?", "QSO B4", "WAIT", "NIL?", "OK?",
];
const CONTROL_NAMES: [&str; 18] = [
    "agn", "call", "agn_call", "nr", "agn_nr", "exch", "state", "section", "zone", "grid", "rprt",
    "qsl_tu", "tu", "qrz", "qso_b4", "wait", "nil", "ok",
];
const SECTIONS: [&str; 86] = [
    "AB", "AK", "AL", "AR", "AZ", "BC", "CO", "CT", "DE", "EB", "EMA", "ENY", "EPA", "EWA", "GA",
    "GH", "IA", "ID", "IL", "IN", "KS", "KY", "LA", "LAX", "NS", "MB", "MDC", "ME", "MI", "MN",
    "MO", "MS", "MT", "NC", "ND", "NE", "NFL", "NH", "NL", "NLI", "NM", "NNJ", "NNY", "TER", "NTX",
    "NV", "OH", "OK", "ONE", "ONN", "ONS", "OR", "ORG", "PAC", "PR", "QC", "RI", "SB", "SC", "SCV",
    "SD", "SDG", "SF", "SFL", "SJV", "SK", "SNJ", "STX", "SV", "TN", "UT", "VA", "VI", "VT", "WCF",
    "WI", "WMA", "WNY", "WPA", "WTX", "WV", "WWA", "WY", "DX", "PE", "NB",
];
const CALL_ACTIONS: [&str; 6] = ["cq", "call", "tu_cq", "call_tu", "call_agn", "tu_now"];
const CALL_RENDERS: [&str; 6] = [
    "CQ %s CQ",
    "%s",
    "TU %s CQ",
    "%s TU",
    "%s AGN?",
    "TU NOW %s",
];
const NUMBER_KINDS: [&str; 8] = [
    "serial",
    "cq_zone",
    "itu_zone",
    "age",
    "power",
    "check",
    "first_license_year",
    "generic_numeric",
];
const LOCATION_KINDS: [&str; 5] = [
    "state_province",
    "arrl_rac_section",
    "country_prefix",
    "generic_qth",
    "local_admin",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageProfile {
    Unknown,
    FieldDay,
    RttyRoundup,
}

#[derive(Clone, Debug, PartialEq)]
struct AtomWord {
    word: u32,
    atom: &'static str,
    subtype: String,
    rendering: String,
    family_rank: i32,
    subtype_rank: i32,
    full_role: bool,
}

type AtomResult = Result<AtomWord, &'static str>;

/// `s[pos..pos + len]`, both clamped to the string (C++ `substr`).
fn sub(s: &str, pos: usize, len: usize) -> &str {
    let pos = pos.min(s.len());
    &s[pos..(pos + len).min(s.len())]
}

fn format_decimal(value: u32, width: usize) -> String {
    format!("{value:0width$}")
}

fn decimal(text: &str) -> Option<u32> {
    let b = text.as_bytes();
    if b.is_empty() || (b.len() > 1 && b[0] == b'0') {
        return None;
    }
    let mut r: u32 = 0;
    for &c in b {
        if !c.is_ascii_digit() {
            return None;
        }
        r = r.checked_mul(10)?.checked_add((c - b'0') as u32)?;
    }
    Some(r)
}

fn base36_value(token: &str) -> Option<u32> {
    if token.len() < 2 || token.len() > 3 {
        return None;
    }
    let mut v = 0u32;
    for c in token.bytes() {
        v = v * 36 + BASE36.iter().position(|&b| b == c)? as u32;
    }
    if (token.len() == 2 && v >= 36 * 36) || (token.len() == 3 && v < 36 * 36) {
        return None;
    }
    Some(v)
}

fn base36_text(mut value: u32, length: usize) -> String {
    let mut r = vec![b'0'; length];
    for i in 0..length {
        r[length - 1 - i] = BASE36[(value % 36) as usize];
        value /= 36;
    }
    String::from_utf8(r).unwrap()
}

fn call_area_digit(call: &[u8]) -> Option<usize> {
    if call.len() < 3 || call.len() > 6 {
        return None;
    }
    for digit in [1usize, 2] {
        if digit >= call.len()
            || !call[digit].is_ascii_digit()
            || digit + 1 >= call.len()
            || call.len() - digit - 1 > 3
        {
            continue;
        }
        let prefix_valid = call[..digit].iter().all(|c| c.is_ascii_alphanumeric());
        let prefix_letter = call[..digit].iter().any(|c| c.is_ascii_uppercase());
        let suffix_valid = call[digit + 1..].iter().all(|c| c.is_ascii_uppercase());
        if prefix_valid && prefix_letter && suffix_valid {
            return Some(digit);
        }
    }
    None
}

fn encode_call(call: &str) -> Option<u32> {
    let digit = call_area_digit(call.as_bytes())?;
    let mut aligned: Vec<u8> = call.bytes().collect();
    if digit == 1 {
        aligned.insert(0, b' ');
    }
    aligned.resize(6, b' ');
    let alphabets = [
        CALL_FIRST,
        CALL_SECOND,
        CALL_DIGIT,
        CALL_SUFFIX,
        CALL_SUFFIX,
        CALL_SUFFIX,
    ];
    let mut value = 0u32;
    for i in 0..6 {
        let at = alphabets[i].iter().position(|&b| b == aligned[i])?;
        value = value * alphabets[i].len() as u32 + at as u32;
    }
    Some(value + CALL_OFFSET)
}

fn decode_call(encoded: u32) -> Option<String> {
    if encoded < CALL_OFFSET {
        return None;
    }
    let mut value = encoded - CALL_OFFSET;
    let radices = [37u32, 36, 10, 27, 27, 27];
    let mut digits = [0usize; 6];
    for i in (0..6).rev() {
        digits[i] = (value % radices[i]) as usize;
        value /= radices[i];
    }
    if value != 0 {
        return None;
    }
    let aligned = [
        CALL_FIRST[digits[0]],
        CALL_SECOND[digits[1]],
        CALL_DIGIT[digits[2]],
        CALL_SUFFIX[digits[3]],
        CALL_SUFFIX[digits[4]],
        CALL_SUFFIX[digits[5]],
    ];
    let s = String::from_utf8(aligned.to_vec()).unwrap();
    let s = s.trim_end_matches(' ');
    let s = s.strip_prefix(' ').unwrap_or(s).to_string();
    (encode_call(&s)? == encoded).then_some(s)
}

fn call_atom(subtype: &str, call: &str) -> AtomResult {
    let action = CALL_ACTIONS
        .iter()
        .position(|&n| n == subtype)
        .ok_or("invalid call action")?;
    let encoded = encode_call(call).ok_or("invalid call")?;
    let rendering = CALL_RENDERS[action].replacen("%s", call, 1);
    Ok(AtomWord {
        word: (encoded << 4) | (((action as u32) & 3) << 2) | (action >= 4) as u32,
        atom: "CALL28",
        subtype: subtype.into(),
        rendering,
        family_rank: 0,
        subtype_rank: action as i32,
        full_role: false,
    })
}

fn number_atom(kind: &str, full: bool, value: u32) -> AtomResult {
    let index = NUMBER_KINDS
        .iter()
        .position(|&n| n == kind)
        .ok_or("invalid numeric kind")?;
    if value > 131_071
        || (index == 1 && !(1..=40).contains(&value))
        || (index == 2 && !(1..=90).contains(&value))
        || (index == 6 && value > 9999)
    {
        return Err("numeric field out of range");
    }
    let width = match index {
        0 => 3,
        1 | 2 | 5 => 2,
        6 => 4,
        _ => 0,
    };
    let mut rendering = format_decimal(value, width);
    if full {
        rendering = format!("599 {rendering}");
    }
    let body = ((full as u32) << 26) | ((index as u32) << 22) | (value << 5);
    Ok(AtomWord {
        word: (body << 5) | 2,
        atom: "EXCH_NUM",
        subtype: kind.into(),
        rendering,
        family_rank: 1,
        subtype_rank: index as i32,
        full_role: full,
    })
}

fn location_atom(kind: &str, full: bool, token: &str) -> AtomResult {
    let index = LOCATION_KINDS
        .iter()
        .position(|&n| n == kind)
        .ok_or("invalid location kind")?;
    let value = base36_value(token).ok_or("noncanonical location token")?;
    let body = ((full as u32) << 26)
        | ((index as u32) << 22)
        | (((token.len() == 3) as u32) << 21)
        | (value << 5);
    let rendering = if full {
        format!("599 {token}")
    } else {
        token.into()
    };
    Ok(AtomWord {
        word: (body << 5) | (1 << 2) | 2,
        atom: "EXCH_LOC",
        subtype: kind.into(),
        rendering,
        family_rank: 2,
        subtype_rank: index as i32,
        full_role: full,
    })
}

fn zone_atom(zone: u32, token: &str) -> AtomResult {
    let value = base36_value(token);
    let (Some(value), true) = (value, (1..=40).contains(&zone)) else {
        return Err("invalid zone/location pair");
    };
    let body = (zone << 18) | (((token.len() == 3) as u32) << 17) | (value << 1);
    Ok(AtomWord {
        word: (body << 5) | (2 << 2) | 2,
        atom: "EXCH_PAIR",
        subtype: "zone_loc3".into(),
        rendering: format!("599 {} {token}", format_decimal(zone, 2)),
        family_rank: 3,
        subtype_rank: 0,
        full_role: true,
    })
}

fn class_atom(count: u32, klass: u8, section: &str) -> AtomResult {
    let found = SECTIONS.iter().position(|&s| s == section);
    let (Some(found), true, true) = (
        found,
        (1..=32).contains(&count),
        (b'A'..=b'F').contains(&klass),
    ) else {
        return Err("invalid class/section");
    };
    let section_id = found as u32 + 1;
    let body = (1 << 24) | (count << 18) | (((klass - b'A') as u32) << 15) | (section_id << 8);
    Ok(AtomWord {
        word: (body << 5) | (2 << 2) | 2,
        atom: "EXCH_PAIR",
        subtype: "class_section".into(),
        rendering: format!("{count}{} {section}", klass as char),
        family_rank: 3,
        subtype_rank: 1,
        full_role: false,
    })
}

fn time_atom(full: bool, serial: u32, minute: u32) -> AtomResult {
    if serial > 16383 || minute > 1439 {
        return Err("serial/time field out of range");
    }
    let body = ((full as u32) << 26) | (serial << 12) | (minute << 1);
    let hhmm = format!(
        "{}{}",
        format_decimal(minute / 60, 2),
        format_decimal(minute % 60, 2)
    );
    let mut rendering = format!("{} {hhmm}", format_decimal(serial, 3));
    if full {
        rendering = format!("599 {rendering}");
    }
    Ok(AtomWord {
        word: (body << 5) | (3 << 2) | 2,
        atom: "EXCH_NUM_TIME",
        subtype: "serial_utc".into(),
        rendering,
        family_rank: 4,
        subtype_rank: 0,
        full_role: full,
    })
}

fn control_atom(phrase: &str) -> AtomResult {
    let id = CONTROLS
        .iter()
        .position(|&c| c == phrase)
        .ok_or("unregistered control phrase")?;
    let body = (id as u32) << 16;
    Ok(AtomWord {
        word: (body << 5) | (4 << 2) | 2,
        atom: "CONTROL",
        subtype: CONTROL_NAMES[id].into(),
        rendering: phrase.into(),
        family_rank: 5,
        subtype_rank: id as i32,
        full_role: false,
    })
}

fn grid_atom(full: bool, grid: &str) -> AtomResult {
    let g = grid.as_bytes();
    if g.len() != 4
        || !(b'A'..=b'R').contains(&g[0])
        || !(b'A'..=b'R').contains(&g[1])
        || !g[2].is_ascii_digit()
        || !g[3].is_ascii_digit()
    {
        return Err("invalid grid");
    }
    let code = (((g[0] - b'A') as u32 * 18 + (g[1] - b'A') as u32) * 10 + (g[2] - b'0') as u32)
        * 10
        + (g[3] - b'0') as u32;
    let body = (1 << 23) | ((full as u32) << 22) | (code << 7);
    let rendering = if full {
        format!("599 {grid}")
    } else {
        grid.into()
    };
    Ok(AtomWord {
        word: (body << 5) | (4 << 2) | 2,
        atom: "GRID4",
        subtype: "grid4".into(),
        rendering,
        family_rank: 6,
        subtype_rank: 0,
        full_role: full,
    })
}

fn text_atom(text: &str, last: bool) -> AtomResult {
    let canonical = if last {
        text.trim_end_matches(' ')
    } else {
        text
    };
    if canonical.is_empty() || canonical.len() > 5 || (!last && canonical.len() != 5) {
        return Err("invalid TEXT5 length");
    }
    let mut body = 0u32;
    for i in 0..5 {
        let ch = canonical.as_bytes().get(i).copied().unwrap_or(b' ');
        let at = alphabet_index(ch).ok_or("unsupported TEXT5 character")?;
        body = (body << 6) | at as u32;
    }
    Ok(AtomWord {
        word: (body << 2) | 3,
        atom: "TEXT5",
        subtype: "literal".into(),
        rendering: canonical.into(),
        family_rank: 7,
        subtype_rank: 0,
        full_role: false,
    })
}

fn text5_chars(word: u32) -> String {
    let chars = word >> 2;
    (0..5)
        .map(|i| ALPHABET[((chars >> (24 - 6 * i)) & 63) as usize] as char)
        .collect()
}

fn decode_word(word: u32, last: bool) -> AtomResult {
    if word == 0 {
        return Err("all-zero grammar sentinel");
    }
    let top = word & 3;
    if top == 3 {
        return text_atom(&text5_chars(word), last);
    }
    if top <= 1 {
        let action = (((word >> 2) & 3) + if top == 1 { 4 } else { 0 }) as usize;
        if action >= 6 {
            return Err("unassigned call action");
        }
        let call = decode_call(word >> 4).ok_or("invalid compact call")?;
        return call_atom(CALL_ACTIONS[action], &call);
    }
    let family = (word >> 2) & 7;
    let body = word >> 5;
    match family {
        0 => {
            if body & 31 != 0 {
                return Err("numeric reserved bits nonzero");
            }
            let role = (body >> 26) & 1 != 0;
            let kind = ((body >> 22) & 15) as usize;
            let value = (body >> 5) & 0x1FFFF;
            if kind >= NUMBER_KINDS.len() {
                return Err("reserved numeric kind");
            }
            number_atom(NUMBER_KINDS[kind], role, value)
        }
        1 => {
            if body & 31 != 0 {
                return Err("location reserved bits nonzero");
            }
            let role = (body >> 26) & 1 != 0;
            let kind = ((body >> 22) & 15) as usize;
            let len = (((body >> 21) & 1) + 2) as usize;
            let value = (body >> 5) & 0xFFFF;
            if kind >= LOCATION_KINDS.len()
                || (len == 2 && value >= 1296)
                || (len == 3 && !(1296..46656).contains(&value))
            {
                return Err("invalid location field");
            }
            location_atom(LOCATION_KINDS[kind], role, &base36_text(value, len))
        }
        2 => {
            if body & 1 != 0 {
                return Err("pair reserved bit nonzero");
            }
            match (body >> 24) & 7 {
                0 => {
                    let zone = (body >> 18) & 63;
                    let len = (((body >> 17) & 1) + 2) as usize;
                    let value = (body >> 1) & 0xFFFF;
                    if (len == 2 && value >= 1296) || (len == 3 && !(1296..46656).contains(&value))
                    {
                        return Err("invalid paired token");
                    }
                    zone_atom(zone, &base36_text(value, len))
                }
                1 => {
                    if body & 0xFF != 0 {
                        return Err("class reserved bits nonzero");
                    }
                    let count = (body >> 18) & 63;
                    let klass = ((body >> 15) & 7) as u8;
                    let section = ((body >> 8) & 127) as usize;
                    if section < 1 || section > SECTIONS.len() {
                        return Err("invalid section");
                    }
                    class_atom(count, b'A' + klass, SECTIONS[section - 1])
                }
                _ => Err("reserved pair schema"),
            }
        }
        3 => {
            if body & 1 != 0 {
                return Err("time reserved bit nonzero");
            }
            time_atom(
                (body >> 26) & 1 != 0,
                (body >> 12) & 0x3FFF,
                (body >> 1) & 0x7FF,
            )
        }
        4 => match (body >> 23) & 15 {
            0 => {
                if body & 0xFFFF != 0 {
                    return Err("control reserved bits nonzero");
                }
                let id = ((body >> 16) & 127) as usize;
                if id >= CONTROLS.len() {
                    return Err("unassigned control");
                }
                control_atom(CONTROLS[id])
            }
            1 => {
                if body & 0x7F != 0 {
                    return Err("grid reserved bits nonzero");
                }
                let role = (body >> 22) & 1 != 0;
                let mut code = (body >> 7) & 0x7FFF;
                if code >= 32400 {
                    return Err("grid code out of range");
                }
                let mut g = [b'0'; 4];
                g[3] = b'0' + (code % 10) as u8;
                code /= 10;
                g[2] = b'0' + (code % 10) as u8;
                code /= 10;
                g[1] = b'A' + (code % 18) as u8;
                code /= 18;
                g[0] = b'A' + code as u8;
                grid_atom(role, std::str::from_utf8(&g).unwrap())
            }
            _ => Err("reserved MISC subtype"),
        },
        _ => Err("reserved STRUCT30 family"),
    }
}

/// One frame's payload checked on its own (the receiver's use): the
/// reserved bit clear and the word valid under its own EOM bit.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedFrameAtom {
    /// "TEXT5" or a structured family name.
    pub atom: &'static str,
    /// Canonical rendering (a final TEXT5 right-trimmed).
    pub rendering: String,
    /// TEXT5 only: the five characters as sent.
    pub text5_raw: String,
    pub end_of_message: bool,
}

pub fn decode_frame_payload(payload: &PayloadBits) -> Result<DecodedFrameAtom, String> {
    if payload.iter().any(|&b| b > 1) {
        return Err("invalid payload bit".into());
    }
    if payload[32] != 0 {
        return Err("reserved payload bit is set".into());
    }
    let eom = payload[33] != 0;
    let word = word_from_payload(payload);
    let atom = decode_word(word, eom)?;
    let text5_raw = if atom.atom == "TEXT5" {
        text5_chars(word)
    } else {
        String::new()
    };
    Ok(DecodedFrameAtom {
        atom: atom.atom,
        rendering: atom.rendering,
        text5_raw,
        end_of_message: eom,
    })
}

/// One frame of a packed message.
#[derive(Clone, Debug, PartialEq)]
pub struct MessageFrame {
    pub atom: &'static str,
    pub subtype: String,
    pub rendering: String,
    pub wire: EncodedFrame,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EncodedMessage {
    pub canonical_text: String,
    pub frames: Vec<MessageFrame>,
}

fn make_frame(atom: &AtomWord, eom: bool) -> Result<MessageFrame, String> {
    let wire = encode_payload(&payload_from_word(atom.word, eom))?;
    Ok(MessageFrame {
        atom: atom.atom,
        subtype: atom.subtype.clone(),
        rendering: atom.rendering.clone(),
        wire,
    })
}

fn normalize_literal(input: &str) -> String {
    let input = &input.as_bytes()[..input.len().min(MAX_MESSAGE_BYTES)];
    let mut out = String::with_capacity(input.len());
    let mut pending = false;
    for &raw in input {
        if raw == b' ' {
            pending = !out.is_empty();
            continue;
        }
        let mut ch = raw.to_ascii_uppercase();
        if alphabet_index(ch).is_none() {
            ch = b'#';
        }
        if pending {
            out.push(' ');
            pending = false;
        }
        out.push(ch as char);
    }
    out
}

fn canonicalize_rtty_serials(mut text: String) -> String {
    let mut at = 0;
    while let Some(found) = text[at..].find("599 ").map(|i| i + at) {
        at = found;
        if at != 0 && text.as_bytes()[at - 1] != b' ' {
            at += 4;
            continue;
        }
        let start = at + 4;
        let end = text[start..].find(' ').map_or(text.len(), |i| i + start);
        let token = &text[start..end];
        if (1..=6).contains(&token.len()) && token.bytes().all(|c| c.is_ascii_digit()) {
            let value: u32 = token.parse().unwrap();
            if value <= 131_071 {
                let replacement = format_decimal(value, 3);
                text.replace_range(start..end, &replacement);
                at = start + replacement.len();
                continue;
            }
        }
        at = start;
    }
    text
}

fn structured_at(source: &str, pos: usize, profile: MessageProfile) -> Vec<AtomWord> {
    let mut result = Vec::new();
    let s = source.as_bytes();
    if pos != 0 && s[pos - 1] != b' ' {
        return result;
    }
    let mut add = |atom: AtomResult| {
        if let Ok(a) = atom
            && sub(source, pos, a.rendering.len()) == a.rendering
        {
            let end = pos + a.rendering.len();
            if end == s.len() || s[end] == b' ' {
                result.push(a);
            }
        }
    };
    let rest = &source[pos..];
    let first = rest.split(' ').next().unwrap_or("");
    add(call_atom("call", first));
    add(call_atom("cq", sub(source, pos + 3.min(s.len() - pos), 6)));
    for len in 3..=6 {
        if pos + len > s.len() {
            break;
        }
        let c = sub(source, pos, len);
        add(call_atom("call", c));
        add(call_atom("call_tu", c));
        add(call_atom("call_agn", c));
    }
    if rest.starts_with("TU ") {
        for len in 3..=6 {
            if pos + 3 + len > s.len() {
                break;
            }
            add(call_atom("tu_cq", sub(source, pos + 3, len)));
        }
    }
    if rest.starts_with("TU NOW ") {
        for len in 3..=6 {
            if pos + 7 + len > s.len() {
                break;
            }
            add(call_atom("tu_now", sub(source, pos + 7, len)));
        }
    }
    if rest.starts_with("CQ ") {
        for len in 3..=6 {
            if pos + 3 + len > s.len() {
                break;
            }
            add(call_atom("cq", sub(source, pos + 3, len)));
        }
    }
    for phrase in CONTROLS {
        add(control_atom(phrase));
    }
    for length in 1..=15 {
        if pos + length > s.len() {
            break;
        }
        let span = sub(source, pos, length);
        if pos + length < s.len() && s[pos + length] != b' ' {
            continue;
        }
        let full = span.starts_with("599 ");
        let field = if full { &span[4..] } else { span };
        if let Some(v) = decimal(field)
            && v <= 131_071
        {
            add(number_atom("generic_numeric", full, v));
        }
        if profile == MessageProfile::RttyRoundup
            && full
            && (3..=6).contains(&field.len())
            && field.bytes().all(|c| c.is_ascii_digit())
        {
            let value: u32 = field.parse().unwrap();
            if value <= 131_071 && field == format_decimal(value, 3) {
                add(number_atom("serial", true, value));
            }
        }
        if full && (2..=3).contains(&field.len()) && field.bytes().any(|c| c.is_ascii_uppercase()) {
            add(location_atom("generic_qth", true, field));
            if profile == MessageProfile::RttyRoundup {
                add(location_atom("state_province", true, field));
            }
        }
        add(grid_atom(full, field));
        if let Some(space) = span.find(' ')
            && !full
        {
            let (left, right) = (&span[..space], &span[space + 1..]);
            let last = left.as_bytes().last().copied().unwrap_or(0);
            if left.len() >= 2
                && (b'A'..=b'F').contains(&last)
                && let Some(count) = decimal(&left[..left.len() - 1])
            {
                add(class_atom(count, last, right));
            }
        }
    }
    result.sort_by(|a, b| (a.word, a.atom, &a.subtype).cmp(&(b.word, b.atom, &b.subtype)));
    result.dedup_by(|b, a| a.word == b.word);
    result
}

#[derive(Clone)]
struct Choice {
    reachable: bool,
    frames: u32,
    atom: Option<AtomWord>,
    next: usize,
    consumed: usize,
}

fn better(frames: u32, atom: &AtomWord, consumed: usize, old: &Choice) -> bool {
    let Some(oa) = old.atom.as_ref().filter(|_| old.reachable) else {
        return true;
    };
    if frames < old.frames {
        return true;
    }
    if frames > old.frames {
        return false;
    }
    let structured = atom.atom != "TEXT5";
    let old_structured = oa.atom != "TEXT5";
    if structured != old_structured {
        return structured;
    }
    if consumed != old.consumed {
        return consumed > old.consumed;
    }
    (atom.family_rank, atom.subtype_rank, atom.full_role)
        < (oa.family_rank, oa.subtype_rank, oa.full_role)
}

/// Text to frames: normalized (upper case, single spaces, characters
/// outside the alphabet as '#', at most 80), then the fewest frames, with
/// structured atoms (calls, numbers, grids, ...) preferred over TEXT5.
pub fn pack_literal_message(
    input: &str,
    profile: MessageProfile,
) -> Result<EncodedMessage, String> {
    if input.len() > 4096 {
        return Err("literal input exceeds 4096-byte bound".into());
    }
    let mut source = normalize_literal(input);
    if source.is_empty() {
        return Err("empty message".into());
    }
    if profile == MessageProfile::RttyRoundup {
        source = canonicalize_rtty_serials(source);
    }
    if source.len() > MAX_MESSAGE_BYTES {
        return Err("canonical message exceeds 80 bytes".into());
    }
    let n = source.len();
    let unreached = Choice {
        reachable: false,
        frames: 999,
        atom: None,
        next: 0,
        consumed: 0,
    };
    let mut dp = vec![unreached; n + 1];
    dp[n].reachable = true;
    dp[n].frames = 0;
    for pos in (0..n).rev() {
        let mut candidates = structured_at(&source, pos, profile);
        let text_len = 5.min(n - pos);
        if (text_len == 5 || pos + text_len == n)
            && let Ok(t) = text_atom(sub(&source, pos, text_len), pos + text_len == n)
        {
            candidates.push(t);
        }
        for atom in candidates {
            let mut consumed = atom.rendering.len();
            if atom.atom != "TEXT5"
                && pos + consumed < n
                && source.as_bytes()[pos + consumed] == b' '
            {
                consumed += 1;
            }
            if pos + consumed > n || !dp[pos + consumed].reachable {
                continue;
            }
            let frames = 1 + dp[pos + consumed].frames;
            if better(frames, &atom, consumed, &dp[pos]) {
                dp[pos] = Choice {
                    reachable: true,
                    frames,
                    atom: Some(atom),
                    next: pos + consumed,
                    consumed,
                };
            }
        }
    }
    if !dp[0].reachable || dp[0].frames as usize > MAX_MESSAGE_FRAMES {
        return Err("message cannot be packed within 16 frames".into());
    }
    let mut frames = Vec::new();
    let mut pos = 0;
    while pos < n {
        let eom = dp[pos].next == n;
        frames.push(make_frame(dp[pos].atom.as_ref().unwrap(), eom)?);
        pos = dp[pos].next;
    }
    Ok(EncodedMessage {
        canonical_text: source,
        frames,
    })
}

/// The waveform of a packed message (`protocol::modulate_frames`).
pub fn modulate_message(
    frames: &[MessageFrame],
    base_frequency_hz: f64,
    amplitude: f64,
) -> Result<Vec<f32>, String> {
    let tones: Vec<_> = frames.iter().map(|f| f.wire.tones).collect();
    crate::protocol::modulate_frames(&tones, base_frequency_hz, amplitude)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atoms(text: &str) -> Vec<(&'static str, String)> {
        pack_literal_message(text, MessageProfile::Unknown)
            .unwrap()
            .frames
            .into_iter()
            .map(|f| (f.atom, f.rendering))
            .collect()
    }

    #[test]
    fn calls_and_grids_pack_as_structured_atoms() {
        assert_eq!(atoms("cq  k1abc fn42"), atoms("CQ K1ABC FN42"));
        // fewest frames first: three TEXT5 here, as in jtty-cuf
        assert_eq!(atoms("CQ K1ABC FN42").len(), 3);
        let a = atoms("K1ABC HELLO");
        assert_eq!(a[0], ("CALL28", "K1ABC".into()));
        assert_eq!(a[1], ("TEXT5", "HELLO".into()));
    }

    #[test]
    fn every_packed_frame_decodes_to_its_rendering() {
        for text in [
            "CQ K1ABC FN42",
            "HELLO, WORLD! 73 DE K1ABC",
            "K1ABC 599 025 1830 TU",
            "AGN? 5A EMA QSL TU",
        ] {
            let m = pack_literal_message(text, MessageProfile::RttyRoundup).unwrap();
            for f in &m.frames {
                let d = decode_frame_payload(&f.wire.payload).unwrap();
                assert_eq!(d.rendering, f.rendering, "{text}");
                assert_eq!(d.atom, f.atom);
            }
            assert!(m.frames.last().unwrap().wire.end_of_message);
        }
    }

    #[test]
    fn calls_round_trip() {
        for call in ["K1ABC", "AB1C", "N0XYZ", "9Z9ZZ", "AA1AAA"] {
            let e = encode_call(call).unwrap();
            assert_eq!(decode_call(e).as_deref(), Some(call));
        }
    }
}
