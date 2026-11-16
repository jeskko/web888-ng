// SPDX-License-Identifier: GPL-3.0-or-later
//! Digital selective calling (ITU-R M.493) on MF/HF: 100 Bd FSK, 170 Hz
//! shift, written from the Recommendation.
//!
//! * Characters: ten bits, seven information bits (least significant
//!   first) and three check bits (most significant first) counting the
//!   information bits that are 0 (B, the higher tone; Y = 1, the lower).
//! * A call: a dot pattern, the phasing sequence (DX positions symbol 125
//!   six times, RX positions 111 down to 104), then each character twice:
//!   the DX copy and, four characters later, the RX copy (DX and RX slots
//!   alternate, so the RX copy is five slots after the DX one). The
//!   format specifier is sent twice; the end of sequence (117, 122, 127)
//!   is followed by the error-check character (ECC: the XOR of the
//!   information characters from one format specifier to the EOS).
//! * Decoding: the phasing is looked for at every bit (two DX and one RX,
//!   two RX and one DX, or three RX right, as §3.3 asks); each character
//!   is its DX copy if that passes its check bits, else its RX copy; where
//!   both fail or disagree, the combination that makes the ECC right is
//!   taken (§10.4). [`Call::summary`] reads the fields out per format.

/// Symbols (Table 3).
pub const PHASING_DX: u8 = 125;
pub const EOS_ACK_RQ: u8 = 117;
pub const EOS_ACK_BQ: u8 = 122;
pub const EOS: u8 = 127;

/// The ten bits of a symbol in transmission order (Y = 1, B = 0).
pub fn encode(sym: u8) -> [u8; 10] {
    let mut b = [0u8; 10];
    let mut zeros = 0;
    for (i, v) in b.iter_mut().take(7).enumerate() {
        *v = (sym >> i) & 1;
        zeros += u8::from(*v == 0);
    }
    b[7] = (zeros >> 2) & 1;
    b[8] = (zeros >> 1) & 1;
    b[9] = zeros & 1;
    b
}

/// The symbol in ten bits, if its check bits agree.
pub fn decode(bits: &[u8]) -> Option<u8> {
    if bits.len() < 10 {
        return None;
    }
    let mut sym = 0u8;
    let mut zeros = 0u8;
    for (i, &v) in bits[..7].iter().enumerate() {
        sym |= (v & 1) << i;
        zeros += u8::from(v & 1 == 0);
    }
    let check = ((bits[7] & 1) << 2) | ((bits[8] & 1) << 1) | (bits[9] & 1);
    (check == zeros).then_some(sym)
}

/// The bit offset (0..9) at which ten-bit words pass their check most
/// often, and that share.
pub fn align(bits: &[u8]) -> (usize, f64) {
    let mut best = (0, 0.0);
    for off in 0..10 {
        let n = bits.len().saturating_sub(off) / 10;
        if n < 8 {
            break;
        }
        let ok = (0..n)
            .filter(|w| decode(&bits[off + 10 * w..]).is_some())
            .count();
        let sh = ok as f64 / n as f64;
        if sh > best.1 {
            best = (off, sh);
        }
    }
    best
}

/// A call's bits as sent: `dots` bits of dot pattern, the phasing, the
/// characters (from the format specifier, given once and sent twice, to
/// the EOS) in DX and RX, and the ECC.
pub fn call_bits(chars: &[u8], dots: usize) -> Vec<u8> {
    let mut info: Vec<u8> = Vec::with_capacity(chars.len() + 2);
    info.push(chars[0]);
    info.extend_from_slice(chars);
    let ecc = chars.iter().fold(0u8, |a, &c| a ^ c);
    info.push(ecc);
    // the slots: phasing (12), then DX j at 2j, RX j at 2j + 5
    let n = info.len();
    let mut slots: Vec<u8> = Vec::new();
    for k in 0..6 {
        slots.push(PHASING_DX);
        slots.push(111 - k as u8);
    }
    let base = slots.len();
    slots.resize(base + 2 * n + 4, 0);
    for (j, &c) in info.iter().enumerate() {
        slots[base + 2 * j] = c;
        slots[base + 2 * j + 5] = c;
    }
    // the two RX phasing characters the message overlaps (105, 104), and
    // DX after the ECC: the EOS twice more (three times in DX in all)
    slots[base + 1] = 105;
    slots[base + 3] = 104;
    let eos = chars[chars.len() - 1];
    for k in [2 * n, 2 * n + 2] {
        slots[base + k] = eos;
    }
    let mut bits: Vec<u8> = (0..dots).map(|i| (i & 1) as u8).collect();
    for s in slots {
        bits.extend_from_slice(&encode(s));
    }
    bits
}

/// A received call: the characters (from the format specifier, once, to
/// the EOS) and the ECC, with how each was read.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub chars: Vec<u8>,
    pub ecc: Option<u8>,
    /// Characters read from one copy only, and ones decided by the ECC.
    pub single: usize,
    pub repaired: usize,
    /// Characters neither copy gave (None in `raw`).
    pub lost: usize,
    pub ecc_ok: bool,
    /// The bit index the format specifier's DX copy starts at.
    pub start: usize,
    /// One past the last bit used.
    pub end: usize,
}

/// Where in a bit stream the phasing says a call's format specifier (DX)
/// starts: the bit index, if §3.3's rule holds there.
fn phased(bits: &[u8], p: usize) -> bool {
    let slot = |k: isize| -> Option<u8> {
        let at = p as isize + 10 * k;
        if at < 0 {
            return None;
        }
        decode(&bits[at as usize..])
    };
    let dx = (1..=6)
        .filter(|&k| slot(-2 * k) == Some(PHASING_DX))
        .count();
    let rx = (0..8)
        .filter(|&j| slot(-11 + 2 * j) == Some(111 - j as u8))
        .count();
    (dx >= 2 && rx >= 1) || (rx >= 2 && dx >= 1) || rx >= 3
}

/// Most characters in a call (a semi-automatic call with a long number).
const MAX_CHARS: usize = 40;

/// The call whose format specifier starts at bit `p`, if the stream holds
/// it all (None: wait for more bits, or nothing there).
fn read_call(bits: &[u8], p: usize) -> Option<Call> {
    let mut dx: Vec<Option<u8>> = Vec::new();
    let mut rx: Vec<Option<u8>> = Vec::new();
    let mut j = 0;
    let mut eos_at = None;
    let mut misses = 0;
    while j < MAX_CHARS {
        let a = p + 20 * j;
        let b = a + 50;
        if b + 10 > bits.len() {
            return None; // not all here yet
        }
        let (d, r) = (decode(&bits[a..]), decode(&bits[b..]));
        dx.push(d);
        rx.push(r);
        let c = d.or(r);
        misses = if c.is_none() { misses + 1 } else { 0 };
        if misses >= 3 {
            break;
        }
        if j >= 2 && [EOS_ACK_RQ, EOS_ACK_BQ, EOS].contains(&c.unwrap_or(0)) && eos_at.is_none() {
            eos_at = Some(j);
            // the ECC follows; its RX copy is the last character sent
            if a + 20 + 60 > bits.len() {
                return None;
            }
            dx.push(decode(&bits[a + 20..]));
            rx.push(decode(&bits[a + 70..]));
            break;
        }
        j += 1;
    }
    let eos = eos_at?;
    // the format specifier is sent twice: chars[0] is the first, the second
    // (index 1) a third copy of it
    let pick = |i: usize| -> (Option<u8>, bool) {
        match (dx[i], rx[i]) {
            (Some(a), Some(b)) if a == b => (Some(a), false),
            (Some(a), None) | (None, Some(a)) => (Some(a), true),
            _ => (None, false),
        }
    };
    let mut chars: Vec<Option<u8>> = Vec::new();
    let mut single = 0;
    let mut doubt: Vec<(usize, Vec<u8>)> = Vec::new();
    // the format specifier from its four copies
    let fs_copies: Vec<u8> = [dx[0], rx[0], dx[1], rx[1]].into_iter().flatten().collect();
    let fs = most_common(&fs_copies);
    chars.push(fs);
    for i in 2..=eos {
        let (c, one) = pick(i);
        if c.is_none() {
            let opts: Vec<u8> = [dx[i], rx[i]].into_iter().flatten().collect();
            doubt.push((chars.len(), opts));
        }
        single += usize::from(one);
        chars.push(c);
    }
    let ecc = pick(eos + 1).0;
    let xor = |cs: &[Option<u8>]| cs.iter().fold(0u8, |a, c| a ^ c.unwrap_or(0));
    let mut repaired = 0;
    if let Some(e) = ecc {
        // characters in doubt (both copies passed but disagreed, or neither
        // did): the choice that makes the ECC right, if exactly one does
        let (both, none): (Vec<_>, Vec<_>) = doubt.iter().partition(|d| d.1.len() == 2);
        if none.is_empty() && !both.is_empty() && both.len() <= 4 {
            let mut fits = Vec::new();
            for m in 0..(1u32 << both.len()) {
                let mut t = chars.clone();
                for (k, d) in both.iter().enumerate() {
                    t[d.0] = Some(d.1[((m >> k) & 1) as usize]);
                }
                if xor(&t) == e {
                    fits.push(t);
                }
            }
            if fits.len() == 1 {
                chars = fits.pop().unwrap_or_default();
                repaired = both.len();
            }
        } else if none.len() == 1 && both.is_empty() {
            // one character lost: the ECC gives it
            let i = none[0].0;
            chars[i] = Some(e ^ xor(&chars));
            repaired = 1;
        }
    }
    let lost = chars.iter().filter(|c| c.is_none()).count();
    let ecc_ok = lost == 0 && ecc == Some(xor(&chars));
    Some(Call {
        chars: chars.iter().map(|c| c.unwrap_or(0)).collect(),
        ecc,
        single,
        repaired,
        lost,
        ecc_ok,
        start: p,
        end: p + 20 * (eos + 1) + 60,
    })
}

fn most_common(v: &[u8]) -> Option<u8> {
    let mut best: Option<(u8, usize)> = None;
    for &a in v {
        let n = v.iter().filter(|&&b| b == a).count();
        if best.is_none_or(|b| n > b.1) {
            best = Some((a, n));
        }
    }
    best.map(|b| b.0)
}

/// Finds calls in a growing bit stream.
#[derive(Default)]
pub struct Scanner {
    /// The next bit index to try as a format specifier's start.
    next: usize,
}

impl Scanner {
    /// Calls completed in `bits` since the last scan. Bits before
    /// [`Scanner::pos`] less a call's phasing (~130 bits) are done with;
    /// a caller that drops bits from the front calls [`Scanner::shift`].
    pub fn scan(&mut self, bits: &[u8]) -> Vec<Call> {
        let mut out = Vec::new();
        // a format specifier needs its phasing before it (12 slots) and
        // at least a few characters after
        while self.next + 200 <= bits.len() {
            let p = self.next;
            if phased(bits, p) {
                match read_call(bits, p) {
                    None if p + 20 * MAX_CHARS + 80 > bits.len() => return out, // wait
                    Some(c) => {
                        self.next = c.end;
                        out.push(c);
                        continue;
                    }
                    None => {}
                }
            }
            self.next += 1;
        }
        out
    }

    /// The scan position (bits before it are done with).
    pub fn pos(&self) -> usize {
        self.next
    }

    /// The caller dropped `n` bits from the front.
    pub fn shift(&mut self, n: usize) {
        self.next = self.next.saturating_sub(n);
    }
}

// ------------------------------------------------------------ the fields

fn digits(cs: &[u8]) -> String {
    cs.iter().map(|c| format!("{c:02}")).collect()
}

/// An MMSI (5 characters: ten digits, the last 0) and what kind of station
/// its leading digits say.
fn mmsi(cs: &[u8]) -> String {
    let d = digits(cs);
    let id = &d[..9.min(d.len())];
    let kind = if id.starts_with("00") {
        "coast"
    } else if id.starts_with('0') {
        "group"
    } else if id.starts_with("111") {
        "SAR aircraft"
    } else if id.starts_with("970") || id.starts_with("972") || id.starts_with("974") {
        "AIS-SART/MOB/EPIRB"
    } else if id.starts_with("99") {
        "aid to navigation"
    } else if id.starts_with('8') {
        "handheld"
    } else {
        "ship"
    };
    if d.len() == 10 && !d.ends_with('0') {
        format!("{id}{} ({kind})", &d[9..])
    } else {
        format!("{id} ({kind})")
    }
}

fn format_name(f: u8) -> &'static str {
    match f {
        102 => "geographic area",
        112 => "distress alert",
        114 => "group",
        116 => "all ships",
        120 => "individual",
        123 => "individual (semi-auto)",
        _ => "format ?",
    }
}

fn category(c: u8) -> &'static str {
    match c {
        100 => "routine",
        108 => "safety",
        110 => "urgency",
        112 => "distress",
        _ => "category ?",
    }
}

fn nature(c: u8) -> &'static str {
    match c {
        100 => "fire, explosion",
        101 => "flooding",
        102 => "collision",
        103 => "grounding",
        104 => "listing, danger of capsizing",
        105 => "sinking",
        106 => "disabled and adrift",
        107 => "undesignated distress",
        108 => "abandoning ship",
        109 => "piracy/armed robbery",
        110 => "man overboard",
        112 => "EPIRB emission",
        _ => "nature ?",
    }
}

fn tc1(c: u8) -> &'static str {
    match c {
        100 => "F3E/G3E all modes TP",
        101 => "F3E/G3E duplex TP",
        103 => "polling",
        104 => "unable to comply",
        105 => "end of call",
        106 => "data",
        109 => "J3E TP",
        110 => "distress acknowledgement",
        112 => "distress relay",
        113 => "F1B/J2B TTY-FEC",
        115 => "F1B/J2B TTY-ARQ",
        118 => "test",
        121 => "position update",
        126 => "no information",
        _ => "telecommand ?",
    }
}

fn tc2(c: u8) -> &'static str {
    match c {
        100 => "no reason given",
        101 => "congestion",
        102 => "busy",
        103 => "queue indication",
        104 => "station barred",
        105 => "no operator available",
        106 => "operator temporarily unavailable",
        107 => "equipment disabled",
        108 => "unable to use proposed channel",
        109 => "unable to use proposed mode",
        110 => "ships and aircraft (Res. 18)",
        111 => "medical transports",
        112 => "pay-phone/public call office",
        113 => "facsimile/data",
        126 => "no information",
        _ => "telecommand ?",
    }
}

fn eos_name(c: u8) -> &'static str {
    match c {
        EOS_ACK_RQ => "ack RQ",
        EOS_ACK_BQ => "ack BQ",
        _ => "EOS",
    }
}

/// Ten position digits: quadrant, latitude (dd mm), longitude (ddd mm).
fn position(cs: &[u8]) -> String {
    let d = digits(cs);
    if d.len() < 10 || d.starts_with("9999999999") {
        return "no position".into();
    }
    let q = &d[..1];
    let (ns, ew) = match q {
        "0" => ('N', 'E'),
        "1" => ('N', 'W'),
        "2" => ('S', 'E'),
        "3" => ('S', 'W'),
        _ => return format!("position {d}?"),
    };
    format!(
        "{}°{}'{ns} {}°{}'{ew}",
        &d[1..3],
        &d[3..5],
        &d[5..8],
        &d[8..10]
    )
}

fn utc(cs: &[u8]) -> String {
    let d = digits(cs);
    if d == "8888" {
        "time n/a".into()
    } else {
        format!("{}:{} UTC", &d[..2], &d[2..4])
    }
}

/// A frequency or channel element (three characters, Table 5 / §8.3.2).
fn freq(cs: &[u8]) -> Option<String> {
    if cs.iter().all(|&c| c == 126) {
        return None;
    }
    let d = digits(cs);
    if d.len() != 6 || cs.iter().any(|&c| c > 99) {
        return Some(format!(
            "? {}",
            cs.iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    Some(match &d[..1] {
        "3" => format!("HF/MF ch {}", d[1..].trim_start_matches('0')),
        "9" => format!("VHF ch {}", d[2..].trim_start_matches('0')),
        "4" => format!("{} (10 Hz units)", &d[1..]),
        _ => {
            let hz100: u32 = d.parse().unwrap_or(0);
            format!("{:.1} kHz", hz100 as f64 / 10.0)
        }
    })
}

impl Call {
    /// The format specifier.
    pub fn format(&self) -> u8 {
        self.chars.first().copied().unwrap_or(0)
    }

    /// One line: the format, category, who to whom, the telecommands or
    /// the distress details, frequencies, the EOS and the checks.
    pub fn summary(&self) -> String {
        let c = &self.chars;
        let n = c.len();
        let eos = c[n - 1];
        let body = &c[1..n - 1];
        let mut parts: Vec<String> = vec![format!("DSC {}", format_name(c[0]))];
        let take = |b: &[u8], i: &mut usize, k: usize| -> Option<Vec<u8>> {
            let v = b.get(*i..*i + k)?.to_vec();
            *i += k;
            Some(v)
        };
        let mut i = 0;
        let distress_info = |b: &[u8], i: &mut usize, parts: &mut Vec<String>| {
            if let Some(v) = take(b, i, 1) {
                parts.push(nature(v[0]).into());
            }
            if let Some(v) = take(b, i, 5) {
                parts.push(position(&v));
            }
            if let Some(v) = take(b, i, 2) {
                parts.push(utc(&v));
            }
            if let Some(v) = take(b, i, 1) {
                parts.push(format!("then {}", tc1(v[0])));
            }
        };
        match c[0] {
            112 => {
                if let Some(v) = take(body, &mut i, 5) {
                    parts.push(format!("from {}", mmsi(&v)));
                }
                distress_info(body, &mut i, &mut parts);
            }
            f => {
                if f != 116
                    && let Some(v) = take(body, &mut i, 5)
                {
                    parts.push(if f == 102 {
                        format!("area {}", position(&v))
                    } else {
                        format!("to {}", mmsi(&v))
                    });
                }
                if let Some(v) = take(body, &mut i, 1) {
                    parts.insert(1, category(v[0]).into());
                }
                if let Some(v) = take(body, &mut i, 5) {
                    parts.push(format!("from {}", mmsi(&v)));
                }
                if let Some(t) = take(body, &mut i, 1) {
                    if t[0] == 110 || t[0] == 112 {
                        // distress acknowledgement or relay: about whom
                        parts.push(tc1(t[0]).into());
                        if let Some(v) = take(body, &mut i, 5) {
                            parts.push(format!("for {}", mmsi(&v)));
                        }
                        distress_info(body, &mut i, &mut parts);
                    } else {
                        let t2 = take(body, &mut i, 1).map(|v| v[0]).unwrap_or(126);
                        let mut tc = tc1(t[0]).to_string();
                        if t2 != 126 {
                            tc += &format!(", {}", tc2(t2));
                        }
                        parts.push(tc);
                        let rest = &body[i.min(body.len())..];
                        if rest.len() >= 6 && rest[0] == 55 {
                            parts.push(format!("at {}", position(&rest[1..6])));
                            if rest.len() >= 8 {
                                parts.push(utc(&rest[6..8]));
                            }
                        } else {
                            let f: Vec<String> = rest
                                .chunks(3)
                                .filter(|ch| ch.len() == 3)
                                .filter_map(freq)
                                .collect();
                            match f.len() {
                                0 => {}
                                1 => parts.push(f[0].clone()),
                                _ => parts.push(format!("RX {} TX {}", f[0], f[1])),
                            }
                        }
                    }
                }
            }
        }
        parts.push(eos_name(eos).into());
        let check = if self.ecc_ok {
            "ECC ok".to_string()
        } else if self.lost > 0 {
            format!("{} characters lost", self.lost)
        } else {
            "ECC wrong".into()
        };
        parts.push(check);
        parts.join(" | ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_matches_table_1() {
        // Table 1: symbol 00 BBBBBBBYYY, 63 YYYYYYBBBY, 125 YBYYYYYBBY, 127 YYYYYYYBBB
        let s = |v: [u8; 10]| {
            v.iter()
                .map(|&b| if b == 1 { 'Y' } else { 'B' })
                .collect::<String>()
        };
        assert_eq!(s(encode(0)), "BBBBBBBYYY");
        assert_eq!(s(encode(63)), "YYYYYYBBBY");
        assert_eq!(s(encode(125)), "YBYYYYYBBY");
        assert_eq!(s(encode(127)), "YYYYYYYBBB");
        assert_eq!(s(encode(104)), "BBBYBYYYBB");
        for v in 0..128u8 {
            assert_eq!(decode(&encode(v)), Some(v));
        }
        let mut b = encode(77);
        b[3] ^= 1;
        assert_eq!(decode(&b), None);
    }

    fn individual() -> Vec<u8> {
        // individual, to coast 001234560, routine, from ship 123456780,
        // J3E TP, no info, RX 8291.0 kHz TX 8291.0 kHz, ack RQ
        vec![
            120, 0, 12, 34, 56, 0, 100, 12, 34, 56, 78, 0, 109, 126, 8, 29, 10, 8, 29, 10,
            EOS_ACK_RQ,
        ]
    }

    #[test]
    fn a_call_is_found_and_read() {
        let mut bits = vec![1u8; 37];
        bits.extend(call_bits(&individual(), 200));
        bits.extend(vec![0u8; 300]);
        let mut sc = Scanner::default();
        let calls = sc.scan(&bits);
        assert_eq!(calls.len(), 1);
        let c = &calls[0];
        assert!(c.ecc_ok && c.single == 0 && c.repaired == 0, "{c:?}");
        assert_eq!(c.chars, individual());
        assert_eq!(
            c.summary(),
            "DSC individual | routine | to 001234560 (coast) | from 123456780 (ship) | J3E TP | RX 8291.0 kHz TX 8291.0 kHz | ack RQ | ECC ok"
        );
    }

    #[test]
    fn errors_in_one_copy_and_both_copies() {
        let call = individual();
        let mut bits = call_bits(&call, 20);
        let p0 = 20 + 120; // the format specifier's DX copy
        // one copy of character 5 wrong (its check fails): the other is used
        bits[p0 + 20 * 6 + 2] ^= 1;
        // both copies of character 9 lost: the ECC gives it back
        bits[p0 + 20 * 10 + 1] ^= 1;
        bits[p0 + 20 * 10 + 50 + 4] ^= 1;
        bits.extend(vec![0u8; 300]);
        let calls = Scanner::default().scan(&bits);
        assert_eq!(calls.len(), 1);
        let c = &calls[0];
        assert_eq!(c.chars, call);
        assert!(c.ecc_ok, "{c:?}");
        assert_eq!((c.single, c.repaired), (1, 1));
    }

    #[test]
    fn distress_alert_reads() {
        // distress from 123456780: sinking, 60°10'N 024°55'E at 10:15 UTC, J3E TP
        let call = vec![
            112, 12, 34, 56, 78, 0, 105, 6, 1, 0, 24, 55, 10, 15, 109, EOS,
        ];
        let mut bits = call_bits(&call, 200);
        bits.extend(vec![1u8; 300]);
        let calls = Scanner::default().scan(&bits);
        assert_eq!(
            calls[0].summary(),
            "DSC distress alert | from 123456780 (ship) | sinking | 60°10'N 024°55'E | 10:15 UTC | then J3E TP | EOS | ECC ok"
        );
    }

    #[test]
    fn phasing_with_most_of_it_lost() {
        let mut bits = call_bits(&individual(), 20);
        // break every DX phasing character but one, and all RX but two
        for k in [0usize, 2, 4, 6, 8, 3, 5, 7, 9, 11] {
            bits[20 + 10 * k] ^= 1;
        }
        bits.extend(vec![0u8; 300]);
        assert_eq!(Scanner::default().scan(&bits).len(), 1);
    }
}
