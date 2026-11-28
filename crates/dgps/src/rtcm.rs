// SPDX-License-Identifier: GPL-3.0-or-later
//! RTCM SC-104 version 2 as DGPS beacons send it: a continuous stream of
//! 30-bit words (24 data bits, most significant first, and 6 parity bits
//! as GPS navigation words have them). A word's data bits go out inverted
//! when the previous word's last bit (D30*) is 1, and the parity chains in
//! the previous word's last two bits, so a stream heard with its polarity
//! flipped decodes the same.
//!
//! A message: header word 1 (preamble 0x66, message type, reference
//! station ID), header word 2 (modified Z-count in 0.6 s since the hour,
//! sequence number, number of data words, station health), then its data
//! words.

/// The header's first 8 bits.
pub const PREAMBLE: u32 = 0x66;

/// The bit of `d` (24 bits) numbered 1-24 from the most significant.
fn bit(d: u32, i: u32) -> u32 {
    (d >> (24 - i)) & 1
}

/// The six parity bits of source data `d` (24 bits, before inversion)
/// after a word ending in `p29`, `p30`.
fn parity_bits(d: u32, p29: u32, p30: u32) -> u32 {
    let x = |ix: &[u32]| ix.iter().fold(0, |a, &i| a ^ bit(d, i));
    let d25 = p29 ^ x(&[1, 2, 3, 5, 6, 10, 11, 12, 13, 14, 17, 18, 20, 23]);
    let d26 = p30 ^ x(&[2, 3, 4, 6, 7, 11, 12, 13, 14, 15, 18, 19, 21, 24]);
    let d27 = p29 ^ x(&[1, 3, 4, 5, 7, 8, 12, 13, 14, 15, 16, 19, 20, 22]);
    let d28 = p30 ^ x(&[2, 4, 5, 6, 8, 9, 13, 14, 15, 16, 17, 20, 21, 23]);
    let d29 = p30 ^ x(&[1, 3, 5, 6, 7, 9, 10, 14, 15, 16, 17, 18, 21, 22, 24]);
    let d30 = p29 ^ x(&[3, 5, 6, 8, 9, 10, 11, 13, 15, 19, 22, 23, 24]);
    (d25 << 5) | (d26 << 4) | (d27 << 3) | (d28 << 2) | (d29 << 1) | d30
}

/// The data (24 bits) of word `w` (30 bits as received) after a word
/// ending in `p29`, `p30`, if its parity holds.
pub fn check(w: u32, p29: u32, p30: u32) -> Option<u32> {
    let d = ((w >> 6) & 0xff_ffff) ^ if p30 == 1 { 0xff_ffff } else { 0 };
    (parity_bits(d, p29, p30) == w & 0x3f).then_some(d)
}

/// Word `d` (24 data bits) as sent after a word ending in `p29`, `p30`.
pub fn encode(d: u32, p29: u32, p30: u32) -> u32 {
    let sent = (d & 0xff_ffff) ^ if p30 == 1 { 0xff_ffff } else { 0 };
    (sent << 6) | parity_bits(d & 0xff_ffff, p29, p30)
}

/// A correction's satellite: (ID, issue of data, UDRE 0-3).
#[derive(Clone, Debug, PartialEq)]
pub struct Sat {
    pub prn: u32,
    pub iod: u32,
    pub udre: u32,
}

/// One beacon of a type 7 almanac.
#[derive(Clone, Debug, PartialEq)]
pub struct Beacon {
    pub lat: f64,
    pub lon: f64,
    /// Range (km).
    pub range: u32,
    /// Frequency (kHz).
    pub khz: f64,
    /// 0 operating, 1 no integrity monitor, 2 no information, 3 don't use.
    pub health: u32,
    /// The broadcasting station's ID.
    pub id: u32,
    pub baud: u32,
}

/// What a message says, where it's decoded (all its words read).
#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    /// Not decoded (another type, or words lost).
    None,
    /// Pseudorange corrections (1, 9: GPS; 31, 34: GLONASS).
    Corr { glonass: bool, sats: Vec<Sat> },
    /// The reference station's position (type 3): WGS84 degrees, metres.
    Pos { lat: f64, lon: f64, h: f64 },
    /// Beacon almanac (type 7).
    Almanac(Vec<Beacon>),
    /// Text (type 16; 36, GLONASS, in KOI8-R).
    Text(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Msg {
    /// Stream time of its first bit (s).
    pub t: f64,
    pub kind: u32,
    /// Reference station ID (0-1023).
    pub station: u32,
    /// Modified Z-count: seconds into the hour (0.6 s steps).
    pub z: f64,
    pub seq: u32,
    /// Station health: 0-5 healthy (UDRE scale 1, 0.75, 0.5, 0.3, 0.2,
    /// 0.1), 6 not monitored, 7 unhealthy.
    pub health: u32,
    /// Data words, None where parity failed.
    pub words: Vec<Option<u32>>,
    pub body: Body,
}

impl Msg {
    pub fn complete(&self) -> bool {
        self.words.iter().all(|w| w.is_some())
    }
}

/// Bits `a..a+n` (n ≤ 32) of the data words strung together.
fn field(words: &[u32], a: usize, n: usize) -> u32 {
    let mut v = 0u32;
    for i in a..a + n {
        let w = words[i / 24];
        v = (v << 1) | ((w >> (23 - i % 24)) & 1);
    }
    v
}

fn signed(v: u32, n: usize) -> i64 {
    let v = i64::from(v);
    if n < 64 && v >= 1 << (n - 1) {
        v - (1 << n)
    } else {
        v
    }
}

/// WGS84 ECEF (m) to latitude, longitude (degrees) and height (m).
pub fn geodetic(x: f64, y: f64, z: f64) -> (f64, f64, f64) {
    let a = 6_378_137.0;
    let f = 1.0 / 298.257_223_563;
    let e2 = f * (2.0 - f);
    let p = x.hypot(y);
    let lon = y.atan2(x);
    let mut lat = z.atan2(p * (1.0 - e2));
    let mut h = 0.0;
    for _ in 0..6 {
        let n = a / (1.0 - e2 * lat.sin().powi(2)).sqrt();
        h = p / lat.cos() - n;
        lat = z.atan2(p * (1.0 - e2 * n / (n + h)));
    }
    (lat.to_degrees(), lon.to_degrees(), h)
}

/// KOI8-R 0xC0-0xFF.
const KOI8R: [char; 64] = [
    'ю', 'а', 'б', 'ц', 'д', 'е', 'ф', 'г', 'х', 'и', 'й', 'к', 'л', 'м', 'н', 'о', //
    'п', 'я', 'р', 'с', 'т', 'у', 'ж', 'в', 'ь', 'ы', 'з', 'ш', 'э', 'щ', 'ч', 'ъ', //
    'Ю', 'А', 'Б', 'Ц', 'Д', 'Е', 'Ф', 'Г', 'Х', 'И', 'Й', 'К', 'Л', 'М', 'Н', 'О', //
    'П', 'Я', 'Р', 'С', 'Т', 'У', 'Ж', 'В', 'Ь', 'Ы', 'З', 'Ш', 'Э', 'Щ', 'Ч', 'Ъ', //
];

/// Bit rates of the almanac's 3-bit code.
const ALM_BAUD: [u32; 8] = [25, 50, 100, 110, 150, 200, 250, 300];

fn body(kind: u32, words: &[Option<u32>]) -> Body {
    let Some(w) = words.iter().copied().collect::<Option<Vec<u32>>>() else {
        return Body::None;
    };
    let bits = w.len() * 24;
    match kind {
        1 | 9 | 31 | 34 => {
            let sats = (0..bits / 40)
                .map(|k| {
                    let a = 40 * k;
                    let prn = field(&w, a + 3, 5);
                    Sat {
                        prn: if prn == 0 && kind < 31 { 32 } else { prn },
                        udre: field(&w, a + 1, 2),
                        iod: field(&w, a + 32, 8),
                    }
                })
                .collect();
            Body::Corr {
                glonass: kind >= 31,
                sats,
            }
        }
        3 if bits >= 96 => {
            let c = |k: usize| signed(field(&w, 32 * k, 32), 32) as f64 * 0.01;
            let (lat, lon, h) = geodetic(c(0), c(1), c(2));
            Body::Pos { lat, lon, h }
        }
        7 => Body::Almanac(
            (0..bits / 72)
                .map(|k| {
                    let a = 72 * k;
                    Beacon {
                        lat: signed(field(&w, a, 16), 16) as f64 * 90.0 / 32768.0,
                        lon: signed(field(&w, a + 16, 16), 16) as f64 * 180.0 / 32768.0,
                        range: field(&w, a + 32, 10),
                        khz: 190.0 + 0.1 * field(&w, a + 42, 12) as f64,
                        health: field(&w, a + 54, 2),
                        id: field(&w, a + 56, 10),
                        baud: ALM_BAUD[field(&w, a + 66, 3) as usize],
                    }
                })
                .collect(),
        ),
        // 16 ASCII; 36 (GLONASS) KOI8-R, Cyrillic from Russian stations
        16 | 36 => Body::Text(
            w.iter()
                .flat_map(|d| [(d >> 16) as u8, (d >> 8) as u8, *d as u8])
                .take_while(|&c| c != 0)
                .map(|c| match c {
                    0x20..0x7f => c as char,
                    0xc0.. if kind == 36 => KOI8R[usize::from(c - 0xc0)],
                    _ => '·',
                })
                .collect(),
        ),
        _ => Body::None,
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Want {
    Hunt,
    Head1,
    Head2,
    Data,
}

/// Bits in, messages out.
pub struct Framer {
    reg: u64,
    nbits: u64,
    want: Want,
    count: u32,
    msg: Option<Msg>,
    n: usize,
    /// Words that passed parity and that failed, while in step.
    pub good: u64,
    pub bad: u64,
    /// Stream time of the last word that passed parity.
    pub last_good: f64,
    /// Bit duration (s).
    bit: f64,
}

impl Framer {
    /// A framer for bits `bit` seconds long.
    pub fn new(bit: f64) -> Self {
        Framer {
            bit,
            reg: 0,
            nbits: 0,
            want: Want::Hunt,
            count: 0,
            msg: None,
            n: 0,
            good: 0,
            bad: 0,
            last_good: f64::NEG_INFINITY,
        }
    }

    /// In step with the words.
    pub fn synced(&self) -> bool {
        self.want != Want::Hunt
    }

    /// The last 30 bits, and the two before them.
    fn word(&self) -> (u32, u32, u32) {
        (
            (self.reg & 0x3fff_ffff) as u32,
            ((self.reg >> 31) & 1) as u32,
            ((self.reg >> 30) & 1) as u32,
        )
    }

    /// A header word 1 at the last 30 bits: starts a message.
    fn head1(&mut self, d: u32, t: f64) {
        self.msg = Some(Msg {
            t,
            kind: (d >> 10) & 63,
            station: d & 1023,
            z: 0.0,
            seq: 0,
            health: 0,
            words: Vec::new(),
            body: Body::None,
        });
        self.want = Want::Head2;
        self.count = 0;
    }

    /// One bit (`t`: its stream time); a message when one ends.
    pub fn push(&mut self, b: bool, t: f64) -> Option<Msg> {
        self.reg = (self.reg << 1) | u64::from(b);
        self.nbits += 1;
        if self.nbits < 32 {
            return None;
        }
        let (w, p29, p30) = self.word();
        // the first bit of this word
        let t0 = t - 29.0 * self.bit;
        if self.want == Want::Hunt {
            if let Some(d) = check(w, p29, p30)
                && d >> 16 == PREAMBLE
            {
                self.head1(d, t0);
            }
            return None;
        }
        self.count += 1;
        if self.count < 30 {
            return None;
        }
        self.count = 0;
        let ok = check(w, p29, p30);
        if ok.is_some() {
            self.good += 1;
            self.last_good = t;
        } else {
            self.bad += 1;
        }
        match self.want {
            Want::Head1 => match ok {
                Some(d) if d >> 16 == PREAMBLE => {
                    self.head1(d, t0);
                }
                _ => {
                    self.want = Want::Hunt;
                    self.msg = None;
                }
            },
            Want::Head2 => match ok {
                Some(d) => {
                    let m = self.msg.as_mut().expect("header 1");
                    m.z = f64::from(d >> 11) * 0.6;
                    m.seq = (d >> 8) & 7;
                    m.health = d & 7;
                    self.n = ((d >> 3) & 31) as usize;
                    if self.n == 0 {
                        self.want = Want::Head1;
                        return self.finish();
                    }
                    self.want = Want::Data;
                }
                None => {
                    self.want = Want::Hunt;
                    self.msg = None;
                }
            },
            Want::Data => {
                let m = self.msg.as_mut().expect("header");
                m.words.push(ok);
                // two words lost in a row: out of step
                let k = m.words.len();
                if k >= 2 && m.words[k - 1].is_none() && m.words[k - 2].is_none() {
                    self.want = Want::Hunt;
                    return self.finish();
                }
                if k == self.n {
                    self.want = Want::Head1;
                    return self.finish();
                }
            }
            Want::Hunt => unreachable!(),
        }
        None
    }

    fn finish(&mut self) -> Option<Msg> {
        let mut m = self.msg.take()?;
        m.body = body(m.kind, &m.words);
        Some(m)
    }
}
