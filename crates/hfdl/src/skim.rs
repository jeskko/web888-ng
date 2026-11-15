// SPDX-License-Identifier: GPL-3.0-or-later
//! One HFDL channel from audio (or IQ) to messages: [`Decoder`] runs the
//! front end and demodulator, parses each burst and describes it, with
//! what it has learnt of each aircraft (its ICAO address from a logon,
//! flight and registration from its messages) filled in.

use crate::demod::Demod;
use crate::front::Front;
use crate::pdu::{self, Body, Hfnpdu, Pdu};
use crate::stations;
use num_complex::Complex32;
use std::collections::HashMap;

/// One part of a burst: a squitter, or an LPDU.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    /// The aircraft (its ID on this ground station; none for a squitter).
    pub ac: Option<u8>,
    /// "squitter", "acars", "position", "frequencies", "logon",
    /// "logon confirm", "logoff", "logon denied", "system table", "data",
    /// "bad" (its CRC failed), "other".
    pub kind: &'static str,
    pub text: String,
    pub flight: String,
    pub reg: String,
    pub icao: Option<u32>,
    pub pos: Option<(f64, f64)>,
}

/// A burst: decoded (`dir` "up", "down" or "squitter") or not (`dir`
/// empty: no CRC passed).
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    /// Stream time (s) of its start.
    pub t: f64,
    pub bps: u32,
    pub double: bool,
    /// Frequency offset (Hz) and SNR (dB).
    pub freq: f32,
    pub snr: f32,
    pub dir: &'static str,
    pub gs: Option<u8>,
    pub items: Vec<Item>,
}

#[derive(Clone, Default)]
struct Aircraft {
    flight: String,
    reg: String,
    /// From the logon confirm that gave the ID.
    icao: Option<u32>,
    /// Stream time last heard of.
    seen: f64,
}

/// What's known of an aircraft is forgotten after this long unheard (s):
/// its ID may have gone to another aircraft (a logoff or logon missed).
const FORGET: f64 = 1800.0;

pub struct Decoder {
    front: Front,
    demod: Demod,
    y: Vec<Complex32>,
    /// (ground station, aircraft ID) → what's known.
    aircraft: HashMap<(u8, u8), Aircraft>,
    pub bursts: u64,
    pub decoded: u64,
}

impl Decoder {
    /// Input at `rate` with the burst's centre at `centre` Hz (audio: the
    /// listed frequency's USB dial gives 1440).
    pub fn new(rate: f64, centre: f64) -> Self {
        Decoder {
            front: Front::new(rate, centre),
            demod: Demod::new(),
            y: Vec::new(),
            aircraft: HashMap::new(),
            bursts: 0,
            decoded: 0,
        }
    }

    pub fn params(&mut self) -> &mut crate::demod::Params {
        &mut self.demod.params
    }

    pub fn false_starts(&self) -> u32 {
        self.demod.false_starts
    }

    /// Stream time (s) of the input so far.
    pub fn now(&self) -> f64 {
        self.front.taken as f64 / self.front_rate()
    }

    fn front_rate(&self) -> f64 {
        self.front.rate()
    }

    pub fn push_audio(&mut self, x: &[f32]) -> Vec<Message> {
        let c: Vec<Complex32> = x.iter().map(|&v| Complex32::new(v, 0.0)).collect();
        self.push(&c)
    }

    pub fn push(&mut self, x: &[Complex32]) -> Vec<Message> {
        self.y.clear();
        self.front.push(x, &mut self.y);
        let bursts = self.demod.push(&self.y);
        bursts
            .into_iter()
            .map(|b| {
                self.bursts += 1;
                let mut m = Message {
                    t: (self.front.time(b.start) - crate::phy::PREKEY as f64 / crate::phy::BAUD)
                        .max(0.0),
                    bps: b.mode.bps(),
                    double: b.mode.double(),
                    freq: b.freq,
                    snr: b.snr,
                    dir: "",
                    gs: None,
                    items: Vec::new(),
                };
                if let Some(p) = pdu::parse(&b.octets) {
                    self.decoded += 1;
                    self.describe(&p, &mut m);
                }
                m
            })
            .collect()
    }

    fn describe(&mut self, p: &Pdu, m: &mut Message) {
        match p {
            Pdu::Squitter(s) => {
                m.dir = "squitter";
                m.gs = Some(s.gs);
                let mut text = format!("TDMA frame {}", s.frame_index);
                for (i, &(id, sync, bits)) in s.stations.iter().enumerate() {
                    if id == 0 {
                        continue;
                    }
                    let f: Vec<String> = stations::freqs(id, bits)
                        .iter()
                        .map(|f| format!("{f:.0}"))
                        .collect();
                    let name = stations::station(id)
                        .map_or(format!("GS {id}"), |s| short(s.name).to_string());
                    let on = if f.is_empty() {
                        "none".into()
                    } else {
                        f.join(", ") + " kHz"
                    };
                    text += &format!("; {}{}: {on}", if i == 0 { "on " } else { "" }, name);
                    if !sync {
                        text += " (no UTC sync)";
                    }
                }
                m.items.push(Item {
                    kind: "squitter",
                    text,
                    ..Default::default()
                });
            }
            Pdu::Mpdu(mp) => {
                m.dir = if mp.uplink { "up" } else { "down" };
                m.gs = Some(mp.gs);
                for l in &mp.lpdus {
                    let mut it = Item {
                        ac: Some(l.ac),
                        ..Default::default()
                    };
                    match &l.body {
                        Body::Data(h) => {
                            it.kind = "data";
                            if let Some(h) = h {
                                it.text = h.to_string();
                                match h {
                                    Hfnpdu::Acars(a) => {
                                        it.kind = "acars";
                                        it.flight = a.flight.clone();
                                        it.reg = a.reg.trim_start_matches('.').to_string();
                                        it.text = acars_text(a);
                                    }
                                    Hfnpdu::Performance {
                                        flight, pos, time, ..
                                    } => {
                                        it.kind = "position";
                                        it.flight = flight.clone();
                                        it.pos = *pos;
                                        it.text = format!(
                                            "{} UTC{}",
                                            hhmmss(*time),
                                            if pos.is_none() { ", no position" } else { "" }
                                        );
                                    }
                                    Hfnpdu::Frequency {
                                        flight, pos, heard, ..
                                    } => {
                                        it.kind = "frequencies";
                                        it.flight = flight.clone();
                                        it.pos = *pos;
                                        let names: Vec<String> = heard
                                            .iter()
                                            .map(|(g, _, _)| {
                                                stations::station(*g)
                                                    .map_or(format!("GS {g}"), |s| {
                                                        short(s.name).into()
                                                    })
                                            })
                                            .collect();
                                        it.text = format!("hears {}", names.join(", "));
                                    }
                                    Hfnpdu::SystemTable { .. } | Hfnpdu::SystemTableRequest => {
                                        it.kind = "system table"
                                    }
                                    _ => {}
                                }
                            }
                        }
                        Body::LogonRequest { icao } => {
                            it.kind = "logon";
                            it.icao = Some(*icao);
                            it.text = if l.kind == 0x4f {
                                "logon resume"
                            } else {
                                "logon request"
                            }
                            .into();
                        }
                        Body::LogonConfirm { icao, id } => {
                            it.kind = "logon confirm";
                            it.icao = Some(*icao);
                            it.text = format!("logged on as aircraft {id}");
                            // the ID is assigned now: what's known goes with it
                            self.aircraft.insert(
                                (mp.gs, *id),
                                Aircraft {
                                    icao: Some(*icao),
                                    seen: m.t,
                                    ..Default::default()
                                },
                            );
                        }
                        Body::Logoff { icao, reason } => {
                            it.kind = "logoff";
                            it.icao = Some(*icao);
                            it.text = format!("logoff ({})", logoff_reason(*reason));
                            let gs = mp.gs;
                            self.aircraft
                                .retain(|k, a| k.0 != gs || a.icao != Some(*icao));
                        }
                        Body::LogonDenied { icao, reason } => {
                            it.kind = "logon denied";
                            it.icao = Some(*icao);
                            it.text = format!("logon denied ({})", denied_reason(*reason));
                        }
                        Body::Bad => {
                            it.kind = "bad";
                            it.text = "(CRC failed)".into();
                        }
                        Body::Unknown(d) => {
                            it.kind = "other";
                            it.text = format!("LPDU type {:02x}, {} octets", l.kind, d.len());
                        }
                    }
                    // what's known of the aircraft, both ways (not for the
                    // logon ID 255, shared by every aircraft logging on, nor
                    // for logons, whose ICAO address is their own)
                    if l.ac != 255 && it.icao.is_none() {
                        let a = self.aircraft.entry((mp.gs, l.ac)).or_default();
                        // a different flight or registration, or long
                        // unheard: another aircraft has the ID now
                        let differs = |mine: &str, known: &str| {
                            !mine.is_empty() && !known.is_empty() && mine != known
                        };
                        if differs(&it.flight, &a.flight)
                            || differs(&it.reg, &a.reg)
                            || m.t - a.seen > FORGET
                        {
                            *a = Aircraft::default();
                        }
                        a.seen = m.t;
                        for (mine, known) in
                            [(&mut it.flight, &mut a.flight), (&mut it.reg, &mut a.reg)]
                        {
                            if mine.is_empty() {
                                mine.clone_from(known);
                            } else {
                                known.clone_from(mine);
                            }
                        }
                        it.icao = a.icao;
                    }
                    m.items.push(it);
                }
            }
        }
    }
}

fn hhmmss(t: u32) -> String {
    format!("{:02}:{:02}:{:02}", t / 3600 % 24, t / 60 % 60, t % 60)
}

/// ARINC 635's logoff reasons.
fn logoff_reason(r: u8) -> &'static str {
    match r {
        1 => "not within slot boundaries",
        2 => "downlink in an uplink slot",
        3 => "RLS protocol error",
        4 => "invalid aircraft ID",
        5 => "no RLS on this ground station",
        6 => "other",
        _ => "reserved",
    }
}

fn denied_reason(r: u8) -> &'static str {
    match r {
        1 => "no aircraft ID free",
        2 => "no RLS on this ground station",
        _ => "reserved",
    }
}

/// "Shannon" from "Shannon, Ireland".
pub fn short(name: &str) -> &str {
    name.split(',').next().unwrap_or(name)
}

fn acars_text(a: &pdu::Acars) -> String {
    let mut t = format!("label {}", a.label);
    if !a.msg.is_empty() {
        t += &format!(" msg {}", a.msg);
    }
    if a.more {
        t += " (continues)";
    }
    if !a.text.is_empty() {
        t += ": ";
        t += &a.text;
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdu::{Acars, Lpdu, Mpdu};

    fn acars(ac: u8, flight: &str, reg: &str) -> Pdu {
        Pdu::Mpdu(Mpdu {
            uplink: false,
            gs: 3,
            lpdus: vec![Lpdu {
                ac,
                kind: 0x0d,
                body: Body::Data(Some(Hfnpdu::Acars(Acars {
                    mode: '2',
                    reg: reg.into(),
                    label: "H1".into(),
                    block: '1',
                    more: false,
                    msg: "M01A".into(),
                    flight: flight.into(),
                    text: String::new(),
                }))),
            }],
        })
    }

    fn message(t: f64) -> Message {
        Message {
            t,
            bps: 300,
            double: false,
            freq: 0.0,
            snr: 0.0,
            dir: "",
            gs: None,
            items: Vec::new(),
        }
    }

    fn who(d: &mut Decoder, p: &Pdu, t: f64) -> (String, String, Option<u32>) {
        let mut m = message(t);
        d.describe(p, &mut m);
        let it = &m.items[0];
        (it.flight.clone(), it.reg.clone(), it.icao)
    }

    #[test]
    fn aircraft_memory() {
        let mut d = Decoder::new(12000.0, 1440.0);
        let confirm = Pdu::Mpdu(Mpdu {
            uplink: true,
            gs: 3,
            lpdus: vec![Lpdu {
                ac: 255,
                kind: 0x9f,
                body: Body::LogonConfirm {
                    icao: 0xABCDEF,
                    id: 10,
                },
            }],
        });
        who(&mut d, &confirm, 0.0);
        // the ID's ICAO address, and its flight and registration learnt
        assert_eq!(
            who(&mut d, &acars(10, "XX0001", "N0CALL"), 10.0),
            ("XX0001".into(), "N0CALL".into(), Some(0xABCDEF))
        );
        assert_eq!(
            who(&mut d, &acars(10, "", ""), 20.0),
            ("XX0001".into(), "N0CALL".into(), Some(0xABCDEF))
        );
        // another ground station's ID 10 is another aircraft
        let mut other = acars(10, "", "");
        if let Pdu::Mpdu(m) = &mut other {
            m.gs = 7;
        }
        assert_eq!(
            who(&mut d, &other, 25.0),
            (String::new(), String::new(), None)
        );
        // a different flight on the ID: another aircraft, nothing carried over
        assert_eq!(
            who(&mut d, &acars(10, "YY0002", ""), 30.0),
            ("YY0002".into(), String::new(), None)
        );
        // long unheard: forgotten
        assert_eq!(
            who(&mut d, &acars(10, "", ""), 30.0 + FORGET + 1.0),
            (String::new(), String::new(), None)
        );
    }
}
