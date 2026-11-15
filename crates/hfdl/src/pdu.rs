// SPDX-License-Identifier: GPL-3.0-or-later
//! A burst's octets to messages (ARINC 635 as dumphfdl and libacars read
//! it): a squitter (SPDU) from a ground station, or an MPDU carrying LPDUs
//! (logons, and data: HFNPDUs with position reports, frequency data,
//! system table pieces, and ACARS messages). Every part has its own CRC;
//! a part that fails is dropped, never guessed at.

use crate::phy::fcs_ok;
use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum Pdu {
    Squitter(Squitter),
    Mpdu(Mpdu),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Squitter {
    pub gs: u8,
    pub utc_sync: bool,
    pub version: u8,
    pub change_note: u8,
    pub frame_index: u16,
    pub frame_offset: u8,
    pub min_priority: u8,
    pub systable_version: u16,
    /// (station, UTC synchronised, frequency bits) for this and two others.
    pub stations: [(u8, bool, u32); 3],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mpdu {
    pub uplink: bool,
    /// Ground station: the source of an uplink, the destination of a
    /// downlink.
    pub gs: u8,
    pub lpdus: Vec<Lpdu>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lpdu {
    /// The aircraft (its ID on this ground station).
    pub ac: u8,
    pub kind: u8,
    pub body: Body,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    Data(Option<Hfnpdu>),
    LogonRequest {
        icao: u32,
    },
    LogonConfirm {
        icao: u32,
        id: u8,
    },
    Logoff {
        icao: u32,
        reason: u8,
    },
    LogonDenied {
        icao: u32,
        reason: u8,
    },
    /// The CRC failed.
    Bad,
    Unknown(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Hfnpdu {
    Performance {
        flight: String,
        pos: Option<(f64, f64)>,
        time: u32,
        gs: u8,
        freq: u8,
    },
    Frequency {
        flight: String,
        pos: Option<(f64, f64)>,
        time: u32,
        /// (station, propagating bits, tuned bits)
        heard: Vec<(u8, u32, u32)>,
    },
    SystemTable {
        version: u16,
        part: u8,
        parts: u8,
    },
    SystemTableRequest,
    DelayedEcho,
    Acars(Acars),
    Other(u8, Vec<u8>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Acars {
    pub mode: char,
    pub reg: String,
    pub label: String,
    pub block: char,
    pub more: bool,
    /// Downlinks: message number and flight.
    pub msg: String,
    pub flight: String,
    pub text: String,
}

pub fn parse(b: &[u8]) -> Option<Pdu> {
    if b.is_empty() {
        return None;
    }
    if b[0] & 1 == 0 {
        squitter(b).map(Pdu::Squitter)
    } else {
        mpdu(b).map(Pdu::Mpdu)
    }
}

fn squitter(b: &[u8]) -> Option<Squitter> {
    if b.len() < 66 || !fcs_ok(b, 64) {
        return None;
    }
    let gs = b[1] & 0x7f;
    Some(Squitter {
        gs,
        utc_sync: b[1] & 0x80 != 0,
        version: (b[0] >> 2) & 3,
        change_note: b[0] >> 6,
        frame_index: b[2] as u16 | ((b[3] as u16 & 0xf) << 8),
        frame_offset: b[3] >> 4,
        min_priority: b[52] & 0xf,
        systable_version: b[53] as u16 | ((b[54] as u16 & 0xf) << 8),
        stations: [
            (
                gs,
                b[1] & 0x80 != 0,
                (b[54] >> 4) as u32 | (b[55] as u32) << 4 | (b[56] as u32) << 12,
            ),
            (
                b[57] & 0x7f,
                b[57] & 0x80 != 0,
                b[58] as u32 | (b[59] as u32) << 8 | (b[60] as u32 & 0xf) << 16,
            ),
            (
                (b[60] >> 4) | ((b[61] & 7) << 4),
                b[61] & 8 != 0,
                (b[61] >> 4) as u32 | (b[62] as u32) << 4 | (b[63] as u32) << 12,
            ),
        ],
    })
}

fn mpdu(b: &[u8]) -> Option<Mpdu> {
    let downlink = b[0] & 2 != 0;
    // (aircraft, LPDU sizes) and the header's length
    let mut groups: Vec<(u8, Vec<usize>)> = Vec::new();
    let hdr = if downlink {
        let n = ((b[0] >> 2) & 0xf) as usize;
        if b.len() < 6 + n {
            return None;
        }
        groups.push((b[2], b[6..6 + n].iter().map(|&s| s as usize + 1).collect()));
        6 + n
    } else {
        let aircraft = (((b[0] & 0x70) >> 4) + 1) as usize;
        let mut h = 2;
        for _ in 0..aircraft {
            if b.len() < h + 2 {
                return None;
            }
            let n = (b[h + 1] >> 4) as usize;
            if b.len() < h + 2 + n {
                return None;
            }
            groups.push((
                b[h],
                b[h + 2..h + 2 + n]
                    .iter()
                    .map(|&s| s as usize + 1)
                    .collect(),
            ));
            h += 2 + n;
        }
        h
    };
    if !fcs_ok(b, hdr) {
        return None;
    }
    let gs = b[1] & 0x7f;
    let mut at = hdr + 2;
    let mut lpdus = Vec::new();
    for (ac, sizes) in groups {
        for s in sizes {
            if at + s > b.len() {
                break;
            }
            lpdus.push(lpdu(ac, &b[at..at + s], !downlink));
            at += s;
        }
    }
    Some(Mpdu {
        uplink: !downlink,
        gs,
        lpdus,
    })
}

/// ICAO addresses are sent with each octet's bits reversed.
fn icao(b: &[u8]) -> u32 {
    b[..3]
        .iter()
        .fold(0u32, |a, &v| (a << 8) | v.reverse_bits() as u32)
}

fn lpdu(ac: u8, b: &[u8], uplink: bool) -> Lpdu {
    let bad = Lpdu {
        ac,
        kind: b.first().copied().unwrap_or(0),
        body: Body::Bad,
    };
    if b.len() < 3 || !fcs_ok(b, b.len() - 2) {
        return bad;
    }
    let d = &b[..b.len() - 2];
    let kind = d[0];
    let body = match kind {
        0x0d | 0x1d => Body::Data(hfnpdu(&d[1..], uplink)),
        0x2f | 0x3f if d.len() >= 5 => {
            let (icao, reason) = (icao(&d[1..]), d[4]);
            if kind == 0x2f {
                Body::LogonDenied { icao, reason }
            } else {
                Body::Logoff { icao, reason }
            }
        }
        0x9f | 0x5f if d.len() >= 5 => Body::LogonConfirm {
            icao: icao(&d[1..]),
            id: d[4],
        },
        0x4f | 0x8f | 0xbf if d.len() >= 4 => Body::LogonRequest {
            icao: icao(&d[1..]),
        },
        _ => Body::Unknown(d.to_vec()),
    };
    Lpdu { ac, kind, body }
}

fn coord(c: u32) -> f64 {
    // 20 bits, two's complement, 180° = 0x7ffff
    let v = ((c << 12) as i32) >> 12;
    v as f64 * 180.0 / 0x7ffff as f64
}

fn position(b: &[u8]) -> Option<(f64, f64)> {
    let lat = coord(b[0] as u32 | (b[1] as u32) << 8 | (b[2] as u32 & 0xf) << 16);
    let lon = coord((b[2] >> 4) as u32 | (b[3] as u32) << 4 | (b[4] as u32) << 12);
    // (180, 180): no position
    (lat.abs() <= 90.0).then_some((lat, lon))
}

fn flight(b: &[u8]) -> String {
    b.iter()
        .map(|&c| {
            if (0x20..0x7f).contains(&c) {
                c as char
            } else {
                '.'
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn hfnpdu(b: &[u8], uplink: bool) -> Option<Hfnpdu> {
    if b.len() < 2 || b[0] != 0xff {
        return None;
    }
    let u16le = |i: usize| b[i] as u32 | (b[i + 1] as u32) << 8;
    Some(match b[1] {
        0xd1 if b.len() >= 47 => Hfnpdu::Performance {
            flight: flight(&b[2..8]),
            pos: position(&b[8..13]),
            time: 2 * u16le(13),
            gs: b[17] & 0x7f,
            freq: b[18],
        },
        0xd5 if b.len() >= 15 => Hfnpdu::Frequency {
            flight: flight(&b[2..8]),
            pos: position(&b[8..13]),
            time: 2 * u16le(13),
            heard: b[15..]
                .as_chunks::<6>()
                .0
                .iter()
                .take(6)
                .map(|c| {
                    (
                        c[0] & 0x7f,
                        c[1] as u32 | (c[2] as u32) << 8 | (c[3] as u32 & 0xf) << 16,
                        (c[3] >> 4) as u32 | (c[4] as u32) << 4 | (c[5] as u32) << 12,
                    )
                })
                .collect(),
        },
        0xd0 if b.len() >= 5 => Hfnpdu::SystemTable {
            version: (b[3] >> 4) as u16 | (b[4] as u16) << 4,
            part: b[2] & 0xf,
            parts: (b[2] >> 4) + 1,
        },
        0xd2 => Hfnpdu::SystemTableRequest,
        0xde => Hfnpdu::DelayedEcho,
        0xff => match acars(&b[2..], uplink) {
            Some(a) => Hfnpdu::Acars(a),
            None => Hfnpdu::Other(0xff, b[2..].to_vec()),
        },
        t => Hfnpdu::Other(t, b[2..].to_vec()),
    })
}

/// An ACARS block: SOH, mode, registration (7), ack, label (2), block ID,
/// [STX, (downlinks: message number (4), flight (6)), text], ETX or ETB,
/// its CRC (2), DEL. Characters carry odd parity in bit 7.
fn acars(b: &[u8], uplink: bool) -> Option<Acars> {
    if b.len() < 17 || b[0] != 1 || *b.last()? != 0x7f {
        return None;
    }
    let s: Vec<u8> = b[1..b.len() - 3].iter().map(|c| c & 0x7f).collect();
    let (&end, s) = s.split_last()?;
    let more = match end {
        3 => false,
        0x17 => true,
        _ => return None,
    };
    let ch = |c: u8| {
        if (0x20..0x7f).contains(&c) {
            c as char
        } else {
            '.'
        }
    };
    let mode = ch(s[0]);
    let reg = flight(&s[1..8]);
    let label: String = [s[9], s[10]]
        .iter()
        .map(|&c| if c == 0x7f { 'd' } else { ch(c) })
        .collect();
    let block = if s[11] == 0 { ' ' } else { ch(s[11]) };
    let mut rest = &s[12..];
    let (mut msg, mut flt) = (String::new(), String::new());
    if let Some((&2, r)) = rest.split_first() {
        rest = r;
        if !uplink && block.is_ascii_digit() && rest.len() >= 10 {
            msg = rest[..4].iter().map(|&c| ch(c)).collect();
            flt = flight(&rest[4..10]);
            rest = &rest[10..];
        }
    }
    let text = rest
        .iter()
        .filter_map(|&c| match c {
            b'\r' => None,
            b'\n' => Some('\n'),
            c => Some(ch(c)),
        })
        .collect();
    Some(Acars {
        mode,
        reg,
        label,
        block,
        more,
        msg,
        flight: flt,
        text,
    })
}

fn hhmmss(t: u32) -> String {
    format!("{:02}:{:02}:{:02}", t / 3600 % 24, t / 60 % 60, t % 60)
}

fn pos(p: &Option<(f64, f64)>) -> String {
    p.map_or("no position".into(), |(a, o)| format!("{a:.4} {o:.4}"))
}

impl fmt::Display for Hfnpdu {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Hfnpdu::Performance {
                flight,
                pos: p,
                time,
                gs,
                freq,
            } => write!(
                f,
                "performance {flight} {} at {} (via GS {gs} freq {freq})",
                pos(p),
                hhmmss(*time)
            ),
            Hfnpdu::Frequency {
                flight,
                pos: p,
                time,
                heard,
            } => {
                write!(
                    f,
                    "frequency data {flight} {} at {}; heard",
                    pos(p),
                    hhmmss(*time)
                )?;
                for (gs, _, _) in heard {
                    write!(f, " {gs}")?;
                }
                Ok(())
            }
            Hfnpdu::SystemTable {
                version,
                part,
                parts,
            } => {
                write!(f, "system table v{version} part {}/{parts}", part + 1)
            }
            Hfnpdu::SystemTableRequest => write!(f, "system table request"),
            Hfnpdu::DelayedEcho => write!(f, "delayed echo"),
            Hfnpdu::Acars(a) => {
                write!(f, "ACARS {} label {} blk {}", a.reg, a.label, a.block)?;
                if !a.flight.is_empty() {
                    write!(f, " flight {} msg {}", a.flight, a.msg)?;
                }
                if a.more {
                    write!(f, " (more)")?;
                }
                if !a.text.is_empty() {
                    write!(f, ": {}", a.text.replace('\n', " | "))?;
                }
                Ok(())
            }
            Hfnpdu::Other(t, b) => write!(f, "HFNPDU {t:02x} ({} octets)", b.len()),
        }
    }
}

impl fmt::Display for Pdu {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Pdu::Squitter(s) => {
                write!(
                    f,
                    "squitter GS {} frame {}.{} systable v{}",
                    s.gs, s.frame_index, s.frame_offset, s.systable_version
                )?;
                for (id, sync, bits) in s.stations {
                    write!(
                        f,
                        "; {id}{} {:?}",
                        if sync { "" } else { " (no UTC)" },
                        crate::stations::freqs(id, bits)
                    )?;
                }
                Ok(())
            }
            Pdu::Mpdu(m) => {
                write!(f, "{} GS {}", if m.uplink { "up" } else { "down" }, m.gs)?;
                for l in &m.lpdus {
                    write!(f, "\n  AC {:3} ", l.ac)?;
                    match &l.body {
                        Body::Data(Some(h)) => write!(f, "{h}")?,
                        Body::Data(None) => write!(f, "data")?,
                        Body::LogonRequest { icao } => write!(f, "logon request {icao:06X}")?,
                        Body::LogonConfirm { icao, id } => {
                            write!(f, "logon confirm {icao:06X} id {id}")?
                        }
                        Body::Logoff { icao, reason } => {
                            write!(f, "logoff {icao:06X} reason {reason}")?
                        }
                        Body::LogonDenied { icao, reason } => {
                            write!(f, "logon denied {icao:06X} reason {reason}")?
                        }
                        Body::Bad => write!(f, "(CRC failed)")?,
                        Body::Unknown(d) => write!(f, "LPDU {:02x} ({} octets)", l.kind, d.len())?,
                    }
                }
                Ok(())
            }
        }
    }
}
