// SPDX-License-Identifier: GPL-3.0-or-later
//! WSPR messages: the 50 message bits, unpacked into types 1-3 with a
//! callsign hash table, packed again, and the 162 channel symbols.
//!
//! Ported from wsprd's `wsprd_utils.c` and `wsprsim_utils.c` (Copyright
//! 2001-2018 Joe Taylor K1JT, 2014-2018 Steven Franke K9AN) and
//! `nhash.c` (K1JT, after Bob Jenkins' public domain lookup3), as
//! distributed with WSJT-X under GPL-3.0-or-later.

/// Sync vector: bit 0 of each channel symbol.
pub const PR3: [u8; 162] = [
    1, 1, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 1, 1, 0, 0, 0, 1, 0, 0, 1, 0, 1, 1, 1, 1, 0, 0, 0, 0, 0,
    0, 0, 1, 0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0, 1, 0, 1, 1, 0, 0, 1, 1, 0, 1, 0, 0, 0, 1, 1, 0, 1, 0,
    0, 0, 0, 1, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 0, 1, 0, 0, 1, 0, 1, 1, 0, 0, 0, 1, 1, 0, 1, 0, 1, 0,
    0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 1, 1, 1, 0, 1, 1, 0, 0, 1, 1, 0, 1, 0, 0, 0, 1, 1, 1,
    0, 0, 0, 0, 0, 1, 0, 1, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 1, 0, 1, 1, 0, 0, 0, 1, 1, 0,
    0, 0,
];

/// Bob Jenkins' lookup3 `hashlittle`, masked to 15 bits (wsprd's nhash).
pub fn nhash(key: &[u8], initval: u32) -> u32 {
    let rot = |x: u32, k: u32| x.rotate_left(k);
    let mut a = 0xdead_beefu32
        .wrapping_add(key.len() as u32)
        .wrapping_add(initval);
    let (mut b, mut c) = (a, a);
    let word = |k: &[u8]| -> u32 {
        k.iter()
            .enumerate()
            .fold(0u32, |w, (i, &x)| w | (x as u32) << (8 * i))
    };
    let mut k = key;
    while k.len() > 12 {
        a = a.wrapping_add(word(&k[0..4]));
        b = b.wrapping_add(word(&k[4..8]));
        c = c.wrapping_add(word(&k[8..12]));
        // mix
        a = a.wrapping_sub(c);
        a ^= rot(c, 4);
        c = c.wrapping_add(b);
        b = b.wrapping_sub(a);
        b ^= rot(a, 6);
        a = a.wrapping_add(c);
        c = c.wrapping_sub(b);
        c ^= rot(b, 8);
        b = b.wrapping_add(a);
        a = a.wrapping_sub(c);
        a ^= rot(c, 16);
        c = c.wrapping_add(b);
        b = b.wrapping_sub(a);
        b ^= rot(a, 19);
        a = a.wrapping_add(c);
        c = c.wrapping_sub(b);
        c ^= rot(b, 4);
        b = b.wrapping_add(a);
        k = &k[12..];
    }
    if k.is_empty() {
        return c;
    }
    a = a.wrapping_add(word(&k[..k.len().min(4)]));
    if k.len() > 4 {
        b = b.wrapping_add(word(&k[4..k.len().min(8)]));
    }
    if k.len() > 8 {
        c = c.wrapping_add(word(&k[8..]));
    }
    // final
    c ^= b;
    c = c.wrapping_sub(rot(b, 14));
    a ^= c;
    a = a.wrapping_sub(rot(c, 11));
    b ^= a;
    b = b.wrapping_sub(rot(a, 25));
    c ^= b;
    c = c.wrapping_sub(rot(b, 16));
    a ^= c;
    a = a.wrapping_sub(rot(c, 4));
    b ^= a;
    b = b.wrapping_sub(rot(a, 14));
    c ^= b;
    c = c.wrapping_sub(rot(b, 24));
    c & 32767
}

/// The hash of a callsign in type 3 messages.
pub fn call_hash(call: &str) -> u32 {
    nhash(call.as_bytes(), 146)
}

/// Callsigns (and their locators) seen in type 1 and 2 messages, by hash.
#[derive(Clone, Default)]
pub struct HashTable {
    calls: std::collections::HashMap<u32, (String, String)>,
}

impl HashTable {
    pub fn get(&self, hash: u32) -> Option<&(String, String)> {
        self.calls.get(&hash)
    }

    fn save(&mut self, call: &str, grid: Option<&str>) {
        let e = self.calls.entry(call_hash(call)).or_default();
        e.0 = call.into();
        if let Some(g) = grid {
            e.1 = g.into();
        }
    }
}

/// A decoded WSPR message.
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    /// As wsprd prints it: "K1ABC FN42 37", "PJ4/K1ABC 37",
    /// "<K1ABC> FN42AX 37" (`<...>` when the hash isn't known).
    pub text: String,
    /// The sender as sent, without the hash brackets; None if unknown.
    pub call: Option<String>,
    pub grid: Option<String>,
    /// Transmitter power, dBm.
    pub dbm: i32,
}

/// n1 (28 bits, the callsign) and n2 (22 bits: locator and power) of the
/// first 50 bits of `dat`.
pub fn unpack50(dat: &[u8]) -> (u32, u32) {
    let d = |i: usize| dat[i] as u32;
    let n1 = d(0) << 20 | d(1) << 12 | d(2) << 4 | (d(3) >> 4) & 15;
    let n2 = (d(3) & 15) << 18 | d(4) << 10 | d(5) << 2 | (d(6) >> 6) & 3;
    (n1, n2)
}

const CHARS: &[u8; 37] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ ";

/// The 6 bytes of a packed callsign (0-padded after the call), or None.
fn unpack_call(ncall: u32) -> Option<[u8; 6]> {
    if ncall >= 262_177_560 {
        return None;
    }
    let mut n = ncall;
    let mut tmp = [0u8; 6];
    for k in (3..6).rev() {
        tmp[k] = CHARS[(n % 27 + 10) as usize];
        n /= 27;
    }
    tmp[2] = CHARS[(n % 10) as usize];
    n /= 10;
    tmp[1] = CHARS[(n % 36) as usize];
    n /= 36;
    tmp[0] = CHARS[n as usize];
    // leading blanks off, then cut at the first blank
    let lead = tmp[..5].iter().take_while(|&&c| c == b' ').count();
    let mut out = [0u8; 6];
    for (o, &c) in out.iter_mut().zip(&tmp[lead..]) {
        if c == b' ' {
            break;
        }
        *o = c;
    }
    Some(out)
}

fn cstr(b: &[u8]) -> String {
    b.iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as char)
        .collect()
}

fn unpack_grid(ngrid: u32) -> Option<String> {
    let ngrid = ngrid >> 7;
    if ngrid >= 32400 {
        return None;
    }
    let dlat = (ngrid % 180) as i32 - 90;
    let mut dlong = (ngrid / 180) as i32 * 2 - 180 + 2;
    if dlong < -180 {
        dlong += 360;
    }
    if dlong > 180 {
        dlong += 360;
    }
    let nlong = (60.0 * (180.0 - dlong as f64) / 5.0) as i32;
    let n1 = nlong / 240;
    let n2 = (nlong - 240 * n1) / 24;
    let nlat = (60.0 * (dlat + 90) as f64 / 2.5) as i32;
    let m1 = nlat / 240;
    let m2 = (nlat - 240 * m1) / 24;
    let c = |i: i32| CHARS[i as usize] as char;
    Some([c(10 + n1), c(10 + m1), c(n2), c(m2)].iter().collect())
}

/// A prefix (`nprefix` < 60000) or suffix added to `call`.
fn unpack_pfx(nprefix: u32, call: &str) -> Option<String> {
    if nprefix < 60000 {
        let mut n = nprefix;
        let mut pfx = [b' '; 3];
        for p in pfx.iter_mut().rev() {
            let nc = (n % 37) as u8;
            *p = match nc {
                0..=9 => nc + 48,
                10..=35 => nc + 55,
                _ => b' ',
            };
            n /= 37;
        }
        // after the last blank
        let start = pfx.iter().rposition(|&c| c == b' ').map_or(0, |p| p + 1);
        Some(format!("{}/{call}", cstr(&pfx[start..])))
    } else {
        // wsprd keeps this in a (signed) char
        let nc = (nprefix - 60000) as u8 as i8 as i32;
        match nc {
            0..=9 => Some(format!("{call}/{}", (nc + 48) as u8 as char)),
            10..=35 => Some(format!("{call}/{}", (nc + 55) as u8 as char)),
            36..=125 => Some(format!(
                "{call}/{}{}",
                ((nc - 26) / 10 + 48) as u8 as char,
                ((nc - 26) % 10 + 48) as u8 as char
            )),
            _ => None,
        }
    }
}

fn power_ok(dbm: i32) -> bool {
    matches!(dbm % 10, 0 | 3 | 7 | 10)
}

/// Unpack the message in the first 50 bits of `dat`, as wsprd's `unpk_`:
/// None for what wsprd doesn't print. Types 1 and 2 go into the table.
pub fn unpack(dat: &[u8], table: &mut HashTable) -> Option<Message> {
    let (n1, n2) = unpack50(dat);
    let call6 = unpack_call(n1)?;
    let grid = unpack_grid(n2)?;
    let callsign = cstr(&call6);
    let ntype = (n2 & 127) as i32 - 64;
    if (0..=62).contains(&ntype) {
        let nu = ntype % 10;
        if nu == 0 || nu == 3 || nu == 7 {
            table.save(&callsign, Some(&grid));
            Some(Message {
                text: format!("{callsign} {grid} {ntype:2}"),
                call: Some(callsign),
                grid: Some(grid),
                dbm: ntype,
            })
        } else {
            let nadd = if nu > 7 {
                nu - 7
            } else if nu > 3 {
                nu - 3
            } else {
                nu
            };
            let n3 = n2 / 128 + 32768 * (nadd as u32 - 1);
            let call = unpack_pfx(n3, &callsign)?;
            let dbm = ntype - nadd;
            if !power_ok(dbm) {
                return None;
            }
            table.save(&call, None);
            Some(Message {
                text: format!("{call} {dbm:2}"),
                call: Some(call),
                grid: None,
                dbm,
            })
        }
    } else if ntype < 0 {
        let dbm = -(ntype + 1);
        // the 6-character locator went in the callsign's place, rotated
        let mut g6 = vec![];
        if call6[5] != 0 {
            g6.push(call6[5]);
        }
        g6.extend(call6[..5].iter().take_while(|&&c| c != 0));
        let ok = power_ok(dbm)
            && g6.len() >= 4
            && g6[0].is_ascii_alphabetic()
            && g6[1].is_ascii_alphabetic()
            && g6[2].is_ascii_digit()
            && g6[3].is_ascii_digit();
        if !ok || ntype == -64 {
            return None;
        }
        let grid6 = cstr(&g6);
        let ihash = (n2 as i32 - ntype - 64) as u32 / 128;
        let known = table.get(ihash).map(|e| e.0.clone());
        let shown = known
            .as_ref()
            .map_or("<...>".to_string(), |c| format!("<{c}>"));
        Some(Message {
            text: format!("{shown} {grid6} {dbm:2}"),
            call: known,
            grid: Some(grid6),
            dbm,
        })
    } else {
        None
    }
}

fn call_code(c: u8) -> u32 {
    match c {
        b'0'..=b'9' => (c - b'0') as u32,
        b'A'..=b'Z' => (c - b'A') as u32 + 10,
        _ => 36,
    }
}

/// n1 of a callsign of up to 6 characters (the digit third, as K1ABC is
/// " K1ABC").
fn pack_call(call: &str) -> Option<u32> {
    let c = call.as_bytes();
    if c.len() > 6 || c.len() < 3 {
        return None;
    }
    let mut c6 = [b' '; 6];
    if c[2].is_ascii_digit() {
        c6[..c.len()].copy_from_slice(c);
    } else if c[1].is_ascii_digit() && c.len() < 6 {
        c6[1..=c.len()].copy_from_slice(c);
    } else {
        return None;
    }
    let v: Vec<u32> = c6.iter().map(|&x| call_code(x)).collect();
    let mut n = v[0];
    n = n * 36 + v[1];
    n = n * 10 + v[2];
    for &x in &v[3..] {
        n = n * 27 + x.checked_sub(10)?;
    }
    Some(n)
}

fn power_round(dbm: i32) -> i32 {
    const NU: [i32; 10] = [0, -1, 1, 0, -1, 2, 1, 0, -1, 1];
    let p = dbm.clamp(0, 60);
    p + NU[(p % 10) as usize]
}

/// Pack a type 1 ("K1ABC FN42 37") or type 3 ("<K1ABC> FN42AX 37")
/// message into its 50 bits (as 7 bytes, the rest 0).
pub fn pack(text: &str) -> Option<[u8; 11]> {
    let parts: Vec<&str> = text.split_whitespace().collect();
    let [call, grid, dbm] = parts[..] else {
        return None;
    };
    let dbm: i32 = dbm.parse().ok()?;
    let g: Vec<u8> = grid.bytes().collect();
    let (n, m) = if let Some(inner) = call.strip_prefix('<').and_then(|c| c.strip_suffix('>')) {
        if g.len() != 6 {
            return None;
        }
        let power = power_round(dbm);
        let ntype = -(power + 1);
        let m = (128 * call_hash(inner) as i32 + ntype + 64) as u32;
        let mut rot = [0u8; 6];
        rot[..5].copy_from_slice(&g[1..6]);
        rot[5] = g[0];
        (pack_call(std::str::from_utf8(&rot).ok()?)?, m)
    } else {
        if g.len() != 4 {
            return None;
        }
        let l = |c: u8| -> i32 {
            match c {
                b'0'..=b'9' => (c - b'0') as i32,
                b'A'..=b'R' => (c - b'A') as i32,
                _ => -1000,
            }
        };
        let m = (179 - 10 * l(g[0]) - l(g[2])) * 180 + 10 * l(g[1]) + l(g[3]);
        if !(0..32400).contains(&m) {
            return None;
        }
        (pack_call(call)?, (m * 128 + dbm + 64) as u32)
    };
    let mut d = [0u8; 11];
    d[0] = (n >> 20) as u8;
    d[1] = (n >> 12) as u8;
    d[2] = (n >> 4) as u8;
    d[3] = (((n & 15) << 4) + ((m >> 18) & 15)) as u8;
    d[4] = (m >> 10) as u8;
    d[5] = (m >> 2) as u8;
    d[6] = ((m & 3) << 6) as u8;
    Some(d)
}

/// The bit-reversal interleaver's order: position `k` of the encoded
/// bits goes to channel symbol `order()[k]`.
fn interleave_order() -> [usize; 162] {
    let mut out = [0usize; 162];
    let mut p = 0;
    let mut i = 0u32;
    while p < 162 {
        let j = (i as u8).reverse_bits() as usize;
        if j < 162 {
            out[p] = j;
            p += 1;
        }
        i += 1;
    }
    out
}

/// Encoded bits from channel order back to code order.
pub fn deinterleave(sym: &mut [u8; 162]) {
    let ord = interleave_order();
    let tmp: [u8; 162] = std::array::from_fn(|p| sym[ord[p]]);
    *sym = tmp;
}

/// The 162 channel symbols (0..4) of a message's 50 bits.
pub fn channel_symbols(data: &[u8; 11]) -> [u8; 162] {
    let bits = crate::fano::encode(data);
    let ord = interleave_order();
    let mut ch = [0u8; 162];
    for p in 0..162 {
        ch[ord[p]] = bits[p];
    }
    std::array::from_fn(|i| 2 * ch[i] + PR3[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_matches_wsprd() {
        // wsprd's nhash(call, len, 146)
        for (call, h) in [
            ("K1ABC", 6521),
            ("DL1STF", 1181),
            ("PJ4/K1ABC", 19735),
            ("G0MBA", 5942),
        ] {
            assert_eq!(call_hash(call), h, "{call}");
        }
    }

    #[test]
    fn round_trips() {
        let mut t = HashTable::default();
        for text in [
            "K1ABC FN42 37",
            "DL1STF JN58 37",
            "G0MBA JO01 23",
            "EB1A IN53 27",
        ] {
            let d = pack(text).unwrap();
            let m = unpack(&d, &mut t).unwrap();
            assert_eq!(m.text, text);
        }
        // type 3: the call is known from the type 1 above
        let d = pack("<K1ABC> FN42AX 37").unwrap();
        let m = unpack(&d, &mut t).unwrap();
        assert_eq!(m.text, "<K1ABC> FN42AX 37");
        assert_eq!(m.call.as_deref(), Some("K1ABC"));
        let m = unpack(&d, &mut HashTable::default()).unwrap();
        assert_eq!(m.text, "<...> FN42AX 37");
    }

    fn symbols(s: &str) -> String {
        s.chars().filter(|c| !c.is_whitespace()).collect()
    }

    /// `wsprsim -cd` from WSJT-X's sources.
    #[test]
    fn matches_wsprsim() {
        let d = pack("K1ABC FN42 37").unwrap();
        assert_eq!(&d[..7], &[0xF7, 0x0C, 0x23, 0x8B, 0x0D, 0x19, 0x40]);
        let s: String = channel_symbols(&d)
            .iter()
            .map(|&v| (b'0' + v) as char)
            .collect();
        assert_eq!(s, symbols(K1ABC));
        let d = pack("<K1ABC> FN42AX 37").unwrap();
        assert_eq!(&d[..7], &[0x9C, 0x36, 0xDB, 0x83, 0x2F, 0x26, 0x80]);
        let s: String = channel_symbols(&d)
            .iter()
            .map(|&v| (b'0' + v) as char)
            .collect();
        assert_eq!(s, symbols(K1ABC_T3));
        // type 2 unpacks
        let d = [0xF7, 0x0C, 0x23, 0x81, 0x0E, 0x99, 0xC0, 0, 0, 0, 0];
        let m = unpack(&d, &mut HashTable::default()).unwrap();
        assert_eq!(m.text, "PJ4/K1ABC 37");
        // and deinterleaving gives the encoder's bits back
        let d = pack("K1ABC FN42 37").unwrap();
        let ch = channel_symbols(&d);
        let mut bits: [u8; 162] = std::array::from_fn(|i| ch[i] >> 1);
        deinterleave(&mut bits);
        assert_eq!(&bits[..], &crate::fano::encode(&d)[..162]);
    }

    const K1ABC: &str = "330020001020131222100323133220200032012322002232110233210221321222033030301210212032132003323032203020201023021112330231212221332000010320132222202332323320031222";
    const K1ABC_T3: &str = "332220023220333220322103133220222012210120222030132213012021103002011232323030210030132021323232201022223221201330130211012021312002210122132020220110101322231200";
}
