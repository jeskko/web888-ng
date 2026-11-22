// SPDX-License-Identifier: GPL-3.0-or-later
//! Time-code stations in the receiver's audio: DCF77, MSF, WWVB and JJY
//! (a keyed LF carrier heard as a tone) and WWV/WWVH (the 100 Hz BCD
//! subcarrier, heard in AM). Written from the stations' published
//! formats (PTB for DCF77, NPL for MSF, NIST SP 432 for WWVB and
//! WWV/WWVH, NICT for JJY).
//!
//! * The tone: a complex mixer at its audio frequency, a 40 ms boxcar
//!   (~25 Hz: an LF carrier on an HF antenna is too weak in a wider one),
//!   its amplitude every 5 ms; impulses blanked before it; divided by its
//!   up level over the 2 s around it (AGC and fades move the level over
//!   seconds, the keying is a ratio within one).
//! * The second's phase: the last 20 s folded into one second and
//!   matched against the station's average keying.
//! * Each second's symbol: its mean level in the station's windows
//!   (scaled by the last 30 s' levels), nearest the symbols' templates
//!   (see [`Station::symbols`]); the margin to the second nearest is its
//!   quality.
//! * Minutes: the alignment whose minute passes the station's checks,
//!   then counted; the date voted over the last minutes; decoded into
//!   [`Minute`]: the UTC the minute starts at, with the stream time it
//!   was heard at, and whether an earlier minute agrees.

pub mod efr;
pub mod rbu;

use num_complex::Complex32;
use std::f64::consts::PI;

/// Frames a second (5 ms each).
const FPS: usize = 200;
const HOP: f64 = 1.0 / FPS as f64;
/// The envelope's boxcar (s).
const WIN: f64 = 0.04;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Station {
    Dcf77,
    Msf,
    Wwvb,
    Jjy,
    /// WWV and WWVH (the same code).
    Wwv,
    /// Phase-modulated, not keyed: its own receiver ([`rbu`]).
    Rbu,
    /// EFR's teleswitch stations: FSK telegrams, a time telegram every
    /// 10 s ([`efr`]). Their carriers aren't steered by atomic clocks.
    Dcf39,
    Hga22,
    Dcf49,
    /// TDF (ALS162), 162 kHz from Allouis: DCF77's code as phase
    /// modulation (a ~50 ms pulse of about -1 rad at each second's start,
    /// a second one at 100 ms for a 1, none in second 59), read by the
    /// envelope receiver from the phase.
    Tdf,
    /// BPC, 68.5 kHz from Shangqiu (Beijing time): the carrier reduced
    /// 0.1-0.4 s at each second's start (one of four lengths, two bits),
    /// none at seconds 0, 20 and 40; three 20 s frames a minute, each with
    /// the whole time and date.
    Bpc,
}

/// Each symbol's mean level (0 keyed down / off, 1 up / on) in the
/// station's windows.
type Templates = &'static [(char, &'static [f32])];

impl Station {
    /// In the page's order (its station numbers).
    pub const ALL: [Station; 11] = [
        Station::Dcf77,
        Station::Msf,
        Station::Wwvb,
        Station::Jjy,
        Station::Wwv,
        Station::Rbu,
        Station::Dcf39,
        Station::Hga22,
        Station::Dcf49,
        Station::Tdf,
        Station::Bpc,
    ];

    /// An EFR teleswitch station ([`efr`]).
    pub fn is_efr(self) -> bool {
        matches!(self, Station::Dcf39 | Station::Hga22 | Station::Dcf49)
    }

    /// Its carrier is an atomic clock's (the ADC clock can be measured on
    /// it).
    pub fn atomic(self) -> bool {
        !self.is_efr()
    }

    /// The keyed carriers (the envelope receiver's).
    pub const KEYED: [Station; 6] = [
        Station::Dcf77,
        Station::Msf,
        Station::Wwvb,
        Station::Jjy,
        Station::Wwv,
        Station::Bpc,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Station::Dcf77 => "DCF77",
            Station::Msf => "MSF",
            Station::Wwvb => "WWVB",
            Station::Jjy => "JJY",
            Station::Wwv => "WWV/WWVH",
            Station::Rbu => "RBU",
            Station::Dcf39 => "DCF39",
            Station::Hga22 => "HGA22",
            Station::Dcf49 => "DCF49",
            Station::Tdf => "TDF",
            Station::Bpc => "BPC",
        }
    }

    pub fn from_name(s: &str) -> Option<Station> {
        Station::ALL.into_iter().find(|v| {
            v.name().eq_ignore_ascii_case(s)
                || (s.eq_ignore_ascii_case("wwv") && *v == Station::Wwv)
        })
    }

    /// The transmitter sending `carrier` (Hz): latitude, longitude (°;
    /// none for WWV/WWVH, two sites on one code).
    pub fn site(self, carrier: f64) -> Option<(f64, f64)> {
        match self {
            // Mainflingen
            Station::Dcf77 => Some((50.0156, 9.0108)),
            // Anthorn
            Station::Msf => Some((54.9116, -3.2785)),
            // Fort Collins
            Station::Wwvb => Some((40.6781, -105.0472)),
            // Otakadoya-yama (40 kHz), Hagane-yama (60 kHz)
            Station::Jjy if carrier < 50_000.0 => Some((37.3725, 140.8489)),
            Station::Jjy => Some((33.4653, 130.1753)),
            Station::Wwv => None,
            // Taldom
            Station::Rbu => Some((56.7333, 37.6633)),
            // Burg near Magdeburg
            Station::Dcf39 | Station::Dcf49 => Some((52.2869, 11.8964)),
            // Lakihegy near Budapest
            Station::Hga22 => Some((47.3725, 19.0042)),
            // Allouis
            Station::Tdf => Some((47.1694, 2.2044)),
            // Shangqiu
            Station::Bpc => Some((34.457, 115.837)),
        }
    }

    /// How far it is usually received (km): its ground wave by day and
    /// sky wave at night; rough, for picking a receiver's stations from
    /// its position (`[timecode] region = "auto"`).
    pub fn reach_km(self) -> f64 {
        match self {
            Station::Dcf77 | Station::Msf | Station::Dcf39 => 2500.0,
            Station::Rbu | Station::Wwvb => 3000.0,
            Station::Bpc => 2500.0,
            Station::Tdf => 2000.0,
            Station::Hga22 | Station::Dcf49 => 1200.0,
            Station::Jjy => 1500.0,
            // HF: not watched by the monitor
            Station::Wwv => 0.0,
        }
    }

    /// The regions it serves (`[timecode] region`).
    pub fn regions(self) -> &'static [&'static str] {
        match self {
            Station::Dcf77
            | Station::Msf
            | Station::Rbu
            | Station::Tdf
            | Station::Dcf39
            | Station::Hga22
            | Station::Dcf49 => &["europe"],
            Station::Wwvb => &["north-america"],
            Station::Jjy | Station::Bpc => &["east-asia"],
            Station::Wwv => &[],
        }
    }

    /// The stations for `region` at a receiver at `rx` (lat, lon °):
    /// "auto" those within their reach of it, a region's name its
    /// stations, "all" every LF one. None: an unknown region, or "auto"
    /// without a position.
    pub fn for_region(region: &str, rx: Option<(f64, f64)>) -> Option<Vec<Station>> {
        let lf = Station::ALL.into_iter().filter(|s| s.reach_km() > 0.0);
        match region {
            "auto" => {
                let rx = rx?;
                Some(
                    lf.filter(|s| {
                        s.carriers().iter().any(|&c| {
                            s.site(c)
                                .is_some_and(|site| distance_km(site, rx) <= s.reach_km())
                        })
                    })
                    .collect(),
                )
            }
            "all" => Some(lf.collect()),
            r => {
                let v: Vec<Station> = lf.filter(|s| s.regions().contains(&r)).collect();
                (!v.is_empty()).then_some(v)
            }
        }
    }

    /// Carrier frequencies (Hz).
    pub fn carriers(self) -> &'static [f64] {
        match self {
            Station::Dcf77 => &[77_500.0],
            Station::Msf => &[60_000.0],
            Station::Wwvb => &[60_000.0],
            Station::Jjy => &[40_000.0, 60_000.0],
            Station::Wwv => &[2.5e6, 5e6, 10e6, 15e6, 20e6, 25e6],
            Station::Rbu => &[200_000.0 / 3.0],
            Station::Dcf39 => &[139_000.0],
            Station::Hga22 => &[135_600.0],
            Station::Dcf49 => &[129_100.0],
            Station::Tdf => &[162_000.0],
            Station::Bpc => &[68_500.0],
        }
    }

    /// The stations a dial frequency may be (within 3 kHz of a carrier).
    pub fn near(hz: f64) -> Vec<Station> {
        Station::ALL
            .into_iter()
            .filter(|s| s.carriers().iter().any(|&c| (c - hz).abs() <= 3000.0))
            .collect()
    }

    /// Windows (s from the second's start) and the symbols' levels in them.
    pub fn symbols(self) -> (&'static [(f64, f64)], Templates) {
        match self {
            // 0.1 s reduced: 0, 0.2 s: 1, none: second 59
            Station::Dcf77 => (
                &[(0.01, 0.09), (0.12, 0.18), (0.25, 0.95)],
                &[
                    ('0', &[0.0, 1.0, 1.0]),
                    ('1', &[0.0, 0.0, 1.0]),
                    ('M', &[1.0, 1.0, 1.0]),
                ],
            ),
            // off 0.1 s, then bit A (0.1-0.2 s off = 1), bit B (0.2-0.3 s);
            // second 0: off 0.5 s. Symbols: '0' A0B0, '1' A1B0, '2' A0B1, '3' A1B1
            Station::Msf => (
                &[
                    (0.01, 0.09),
                    (0.12, 0.18),
                    (0.22, 0.28),
                    (0.33, 0.47),
                    (0.55, 0.95),
                ],
                &[
                    ('0', &[0.0, 1.0, 1.0, 1.0, 1.0]),
                    ('1', &[0.0, 0.0, 1.0, 1.0, 1.0]),
                    ('2', &[0.0, 1.0, 0.0, 1.0, 1.0]),
                    ('3', &[0.0, 0.0, 0.0, 1.0, 1.0]),
                    ('M', &[0.0, 0.0, 0.0, 0.0, 1.0]),
                ],
            ),
            // reduced 0.2 s: 0, 0.5 s: 1, 0.8 s: marker
            Station::Wwvb => (
                &[(0.03, 0.17), (0.23, 0.47), (0.53, 0.77), (0.83, 0.97)],
                &[
                    ('0', &[0.0, 1.0, 1.0, 1.0]),
                    ('1', &[0.0, 0.0, 1.0, 1.0]),
                    ('M', &[0.0, 0.0, 0.0, 1.0]),
                ],
            ),
            // on 0.8 s: 0, 0.5 s: 1, 0.2 s: marker
            Station::Jjy => (
                &[(0.03, 0.17), (0.23, 0.47), (0.53, 0.77), (0.83, 0.97)],
                &[
                    ('0', &[1.0, 1.0, 1.0, 0.0]),
                    ('1', &[1.0, 1.0, 0.0, 0.0]),
                    ('M', &[1.0, 0.0, 0.0, 0.0]),
                ],
            ),
            // 100 Hz on from 0.03 s: 0.17 s: 0, 0.47 s: 1, 0.77 s: marker;
            // second 0: none (the hole)
            Station::Wwv => (
                &[(0.06, 0.17), (0.23, 0.47), (0.53, 0.77), (0.83, 0.97)],
                &[
                    ('0', &[1.0, 0.0, 0.0, 0.0]),
                    ('1', &[1.0, 1.0, 0.0, 0.0]),
                    ('M', &[1.0, 1.0, 1.0, 0.0]),
                    ('H', &[0.0, 0.0, 0.0, 0.0]),
                ],
            ),
            // the phase pulse (read as a low) at 0 ms: 0; at 0 and 100 ms:
            // 1; none: second 59
            // (150-800 ms carry other phase modulation, every second: the
            // up window is after it)
            // (a cycle of +-0.9 rad centred on 0 ms; a 1's centred on 100
            // ms: its two lobes)
            Station::Tdf => (
                &[(0.01, 0.04), (0.06, 0.09), (0.11, 0.14), (0.86, 0.93)],
                &[
                    ('0', &[0.0, 1.0, 1.0, 1.0]),
                    ('1', &[0.0, 0.0, 0.0, 1.0]),
                    ('M', &[1.0, 1.0, 1.0, 1.0]),
                ],
            ),
            // reduced 0.1 s: 0, 0.2 s: 1, 0.3 s: 2, 0.4 s: 3; none: the
            // frame's marker (seconds 0, 20, 40)
            Station::Bpc => (
                &[
                    (0.02, 0.08),
                    (0.12, 0.18),
                    (0.22, 0.28),
                    (0.32, 0.38),
                    (0.5, 0.95),
                ],
                &[
                    ('0', &[0.0, 1.0, 1.0, 1.0, 1.0]),
                    ('1', &[0.0, 0.0, 1.0, 1.0, 1.0]),
                    ('2', &[0.0, 0.0, 0.0, 1.0, 1.0]),
                    ('3', &[0.0, 0.0, 0.0, 0.0, 1.0]),
                    ('M', &[1.0, 1.0, 1.0, 1.0, 1.0]),
                ],
            ),
            Station::Rbu | Station::Dcf39 | Station::Hga22 | Station::Dcf49 => (&[], &[]),
        }
    }

    /// The average level over a second (200 frames), symbols weighed as
    /// sent (for the phase).
    fn profile(self) -> [f32; FPS] {
        let mut p = [0f32; FPS];
        // TDF's marker as read (a KiwiSDR near Paris, 2026-11-21): the
        // phase's |deviation| in two lobes of ~0.9 rad, 40 ms either side
        // of the second's start; a 1's at 100 ms; other modulation over
        // 150-800 ms
        let tri = |t: f64| -> f32 {
            if (0.005..0.045).contains(&t) {
                (1.0 - (t - 0.025).abs() / 0.02) as f32
            } else {
                0.0
            }
        };
        for (k, v) in p.iter_mut().enumerate() {
            let t = k as f64 * HOP;
            *v = match self {
                Station::Tdf => {
                    let busy = if (0.15..0.8).contains(&t) { 0.15 } else { 0.0 };
                    1.0 - 0.9 * tri(t)
                        - 0.9 * tri(t - 0.95)
                        - 0.45 * tri(t - 0.05)
                        - 0.45 * tri(t - 0.1)
                        - busy
                }
                Station::Dcf77 => {
                    if t < 0.1 {
                        0.02
                    } else if t < 0.2 {
                        0.6
                    } else {
                        1.0
                    }
                }
                Station::Msf => {
                    if t < 0.1 {
                        0.02
                    } else if t < 0.3 {
                        0.6
                    } else if t < 0.5 {
                        0.98
                    } else {
                        1.0
                    }
                }
                Station::Wwvb => {
                    if t < 0.2 {
                        0.0
                    } else if t < 0.5 {
                        0.5
                    } else if t < 0.8 {
                        0.9
                    } else {
                        1.0
                    }
                }
                Station::Jjy => {
                    if t < 0.2 {
                        1.0
                    } else if t < 0.5 {
                        0.9
                    } else if t < 0.8 {
                        0.5
                    } else {
                        0.0
                    }
                }
                Station::Wwv => {
                    if t < 0.03 {
                        0.0
                    } else if t < 0.2 {
                        0.98
                    } else if t < 0.5 {
                        0.5
                    } else if t < 0.8 {
                        0.1
                    } else {
                        0.0
                    }
                }
                Station::Bpc => {
                    if t < 0.1 {
                        0.05
                    } else if t < 0.2 {
                        0.4
                    } else if t < 0.3 {
                        0.65
                    } else if t < 0.4 {
                        0.85
                    } else {
                        1.0
                    }
                }
                Station::Rbu | Station::Dcf39 | Station::Hga22 | Station::Dcf49 => 1.0,
            };
        }
        p
    }

    /// What a symbol costs on top of its distance: a minute's marker is
    /// sent once (DCF77, MSF, WWV's hole) or 7 times a minute (WWVB, JJY),
    /// so noise shouldn't read one where a bit was as easily.
    fn rarity(self, c: char) -> f32 {
        let r = 0.1;
        match (self, c) {
            (Station::Dcf77 | Station::Msf | Station::Tdf, 'M') | (Station::Wwv, 'H') => r,
            (Station::Wwvb | Station::Jjy, 'M') => r / 3.0,
            (Station::Bpc, 'M') => r / 2.0,
            _ => 0.0,
        }
    }

    /// Seconds whose symbols stay the same all day (the date, year,
    /// zone): voted over the last minutes.
    fn date_seconds(self) -> &'static [std::ops::RangeInclusive<usize>] {
        match self {
            Station::Dcf77 | Station::Tdf => &[17..=18, 36..=58],
            Station::Msf => &[17..=38, 54..=56],
            Station::Wwvb => &[22..=33, 45..=53],
            Station::Jjy => &[22..=33, 41..=48, 50..=52],
            Station::Wwv => &[4..=7, 30..=41, 51..=54],
            // each frame's day, month, year and their parity
            Station::Bpc => &[11..=19, 31..=39, 51..=59],
            Station::Rbu | Station::Dcf39 | Station::Hga22 | Station::Dcf49 => &[],
        }
    }

    /// The share of time keyed down (for the levels' percentiles).
    fn down(self) -> f32 {
        match self {
            Station::Dcf77 => 0.15,
            Station::Tdf => 0.06,
            Station::Msf | Station::Bpc => 0.25,
            Station::Wwvb | Station::Jjy => 0.5,
            Station::Wwv => 0.65,
            Station::Rbu | Station::Dcf39 | Station::Hga22 | Station::Dcf49 => 0.05,
        }
    }

    /// The UTC offset of the time sent (s), if fixed (DCF77 and MSF say
    /// theirs per minute).
    fn zone(self) -> i64 {
        match self {
            Station::Jjy => 9 * 3600,
            Station::Bpc => 8 * 3600,
            _ => 0,
        }
    }
}

/// A decoded minute.
#[derive(Clone, Debug, PartialEq)]
pub struct Minute {
    pub station: Station,
    /// Stream time (s) the minute starts at (its second 0).
    pub t: f64,
    /// UTC of that instant (Unix seconds).
    pub utc: i64,
    /// The station's own time, as it says it ("2026-11-16 11:24 CET").
    pub local: String,
    /// Parity and range checks passed (the date voted over the last
    /// minutes, see [`Station::date_seconds`]).
    pub ok: bool,
    /// Its time of day agrees with an earlier minute that passed.
    pub confirmed: bool,
    /// Announcements and corrections (DUT1, summer time, leap second).
    pub notes: Vec<String>,
    /// The minute's symbols as read ('?' unread).
    pub symbols: String,
}

/// A second read.
#[derive(Clone, Debug, PartialEq)]
pub struct Second {
    /// Stream time (s) it starts at.
    pub t: f64,
    pub sym: char,
    /// The margin between the nearest template and the next (0..1).
    pub quality: f32,
    /// Its index in the minute, once the minute's start is known.
    pub index: Option<u8>,
    /// Its level (0 down .. 1 up) in each of the station's windows.
    pub levels: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Second(Second),
    Minute(Minute),
}

/// The receiver for one station, its tone at an audio frequency.
pub struct Receiver {
    station: Station,
    fs: f64,
    t0: f64,
    /// Mixer: e^{-jφ} and its step per sample (a rotator: no sine or
    /// cosine per sample, back on the unit circle after each push).
    lo: num_complex::Complex64,
    lo_step: num_complex::Complex64,
    /// The boxcar: the last WIN of mixed samples, and their sum.
    ring: Vec<Complex32>,
    pos: usize,
    sum: num_complex::Complex64,
    hop: usize,
    hop_n: usize,
    /// TDF: this hop's carrier, summed, and the slow reference its phase
    /// is measured against (~1 s).
    tdf_acc: num_complex::Complex64,
    tdf_ref: num_complex::Complex64,
    n: u64,
    /// The envelope (amplitude per 5 ms frame) as measured, and divided by
    /// its up level over the 2 s around it (`env`, a second behind `raw`):
    /// AGC recovering from an impulse, or a fade, moves the level over
    /// seconds; the keying is a ratio within one. Frames env_base.. .
    raw: Vec<f32>,
    env: Vec<f32>,
    env_base: u64,
    /// The normalising window's values, sorted, and its span of `raw`
    /// (absolute frames: start, end): it slides a frame at a time, so one
    /// value leaves and one enters (an order statistic of 400 values per
    /// frame was most of the decoder's time on the unit).
    sorted: Vec<f32>,
    win: (u64, u64),
    /// The second's phase (frame index mod FPS) and when it was set.
    phase: Option<usize>,
    phase_at: u64,
    /// The true phase's offset from `phase` (frames, within +-4): the
    /// correlation peak interpolated. Seconds are read at whole frames
    /// (5 ms); their times carry this, or a station's lateness steps by
    /// 5 ms as the phase drifts across a frame.
    phase_off: f64,
    /// The next second to read: its first frame (absolute).
    next: Option<u64>,
    syms: Vec<Second>,
    /// The index in `syms` where the current minute starts.
    minute_at: Option<usize>,
    out: Vec<Event>,
    pub level: f32,
    /// The noise blanker's average |x|.
    avg: f64,
    /// Minutes in a row that failed (the minute's lock is dropped at 2).
    bad: u32,
    /// The last minutes' frames (for the date's vote) and the minutes
    /// that passed (stream time, UTC).
    frames: std::collections::VecDeque<Vec<Second>>,
    passed: Vec<(f64, i64)>,
    /// RBU: its own receiver does the work.
    rbu: Option<Box<rbu::Rbu>>,
    /// EFR: its own receiver too.
    efr: Option<Box<efr::Efr>>,
}

impl Receiver {
    /// `tone`: the carrier's (or WWV's 100 Hz subcarrier's) audio
    /// frequency (Hz); `t0`: the stream time of the first sample.
    pub fn new(station: Station, fs: f64, tone: f64, t0: f64) -> Self {
        let w = (WIN * fs).round() as usize;
        Receiver {
            station,
            fs,
            t0,
            lo: num_complex::Complex64::new(1.0, 0.0),
            lo_step: num_complex::Complex64::from_polar(1.0, -2.0 * PI * tone / fs),
            ring: vec![Complex32::new(0.0, 0.0); w],
            pos: 0,
            sum: num_complex::Complex64::new(0.0, 0.0),
            hop: (fs * HOP).round() as usize,
            hop_n: 0,
            tdf_acc: num_complex::Complex64::new(0.0, 0.0),
            tdf_ref: num_complex::Complex64::new(0.0, 0.0),
            n: 0,
            raw: Vec::new(),
            env: Vec::new(),
            env_base: 0,
            sorted: Vec::new(),
            win: (0, 0),
            phase: None,
            phase_at: 0,
            phase_off: 0.0,
            next: None,
            syms: Vec::new(),
            minute_at: None,
            out: Vec::new(),
            level: 0.0,
            avg: 0.0,
            bad: 0,
            frames: std::collections::VecDeque::new(),
            passed: Vec::new(),
            rbu: (station == Station::Rbu).then(|| Box::new(rbu::Rbu::new(fs, tone, t0))),
            efr: station
                .is_efr()
                .then(|| Box::new(efr::Efr::new(station, fs, tone, t0))),
        }
    }

    pub fn station(&self) -> Station {
        self.station
    }

    /// Retune the tone (Hz).
    pub fn set_tone(&mut self, tone: f64) {
        if let Some(r) = &mut self.rbu {
            r.set_tone(tone);
        }
        if let Some(r) = &mut self.efr {
            r.set_tone(tone);
        }
        self.lo_step = num_complex::Complex64::from_polar(1.0, -2.0 * PI * tone / self.fs);
    }

    /// Stream time of envelope frame k (the boxcar's centre).
    fn frame_t(&self, k: u64) -> f64 {
        self.t0 + (k as f64 + 1.0) * HOP - 0.5 * WIN
    }

    pub fn push(&mut self, x: &[f32]) {
        if let Some(r) = &mut self.rbu {
            r.push(x);
            self.level = r.level;
            return;
        }
        if let Some(r) = &mut self.efr {
            r.push(x);
            self.level = r.level;
            return;
        }
        let w = self.ring.len();
        // the blanker: an impulse (|x| over 6 times the average) is zeroed.
        // The average follows every sample, capped at 4 times the
        // threshold: one impulse raises it by ~5 %, and a carrier back at
        // full power after a long low (WWVB's 0.8 s at -17 dB, JJY's
        // -20 dB, MSF's 0.5 s off: 40 dB and more at a strong site) is
        // let through within ~30 ms instead of being blanked for good
        let a_ema = 1.0 / (0.2 * self.fs);
        for &v in x {
            let m = (v as f64).abs();
            let thr = 6.0 * self.avg / 0.64;
            let blank = self.n > (self.fs as u64) && m > thr;
            let m_avg = if self.n > (self.fs as u64) {
                m.min(4.0 * thr)
            } else {
                m
            };
            self.avg += (m_avg - self.avg) * a_ema;
            let v = if blank { 0.0 } else { v };
            let z = Complex32::new(
                (v as f64 * self.lo.re) as f32,
                (v as f64 * self.lo.im) as f32,
            );
            self.lo *= self.lo_step;
            let old = self.ring[self.pos];
            self.ring[self.pos] = z;
            self.pos = (self.pos + 1) % w;
            self.sum += num_complex::Complex64::new((z.re - old.re) as f64, (z.im - old.im) as f64);
            self.n += 1;
            self.hop_n += 1;
            if self.station == Station::Tdf {
                self.tdf_acc += num_complex::Complex64::new(z.re as f64, z.im as f64);
            }
            if self.hop_n == self.hop {
                self.hop_n = 0;
                if self.station == Station::Tdf {
                    // the phase pulses as lows: 1 - |deviation| (rad)
                    // against the carrier's phase over the last second
                    let p = std::mem::take(&mut self.tdf_acc);
                    let a = HOP / 1.0;
                    self.tdf_ref = self.tdf_ref * (1.0 - a) + p * a;
                    let d = (p * self.tdf_ref.conj()).arg().abs();
                    self.raw.push((1.0 - d.min(1.0) * 0.98) as f32);
                } else {
                    self.raw.push((self.sum.norm() / w as f64) as f32);
                }
            }
        }
        self.lo /= self.lo.norm();
        // re-sum now and then (rounding)
        if self.n % (self.fs as u64 * 10) < x.len() as u64 {
            self.sum = self
                .ring
                .iter()
                .map(|c| num_complex::Complex64::new(c.re as f64, c.im as f64))
                .sum();
        }
        // normalise: frames with a second of envelope after them
        let h = FPS;
        let base = self.env_base;
        while self.env.len() + h < self.raw.len() {
            let k = self.env.len();
            let a = k.saturating_sub(h);
            // the window raw[a..k + h], sorted
            let (a_abs, b_abs) = (base + a as u64, base + (k + h) as u64);
            while self.win.1 < b_abs {
                let v = self.raw[(self.win.1 - base) as usize];
                let i = self.sorted.partition_point(|x| x.total_cmp(&v).is_le());
                self.sorted.insert(i, v);
                self.win.1 += 1;
            }
            while self.win.0 < a_abs {
                let v = self.raw[(self.win.0 - base) as usize];
                let i = self.sorted.partition_point(|x| x.total_cmp(&v).is_lt());
                self.sorted.remove(i);
                self.win.0 += 1;
            }
            // the up level: the station's percentile (DCF77's carrier is up
            // 85 % of the time, WWVB's about half)
            let n = self.sorted.len();
            let dn = self.station.down();
            let q = (((dn + (1.0 - dn) / 2.0) * n as f32) as usize).min(n - 1);
            let up = self.sorted[q];
            self.env.push(self.raw[k] / up.max(1e-30));
        }
        self.read();
        // keep 40 s
        let keep = 40 * FPS;
        if self.env.len() > keep + 2 * FPS {
            let d = self.env.len() - keep;
            self.env.drain(..d);
            self.raw.drain(..d);
            self.env_base += d as u64;
        }
    }

    pub fn poll(&mut self) -> Vec<Event> {
        if let Some(r) = &mut self.rbu {
            return r.poll();
        }
        if let Some(r) = &mut self.efr {
            return r.poll();
        }
        std::mem::take(&mut self.out)
    }

    /// The levels keyed down and up over the last `secs`: the middle of
    /// each part by the station's share of time keyed down.
    fn scale(&self, secs: usize) -> Option<(f32, f32)> {
        let n = (secs * FPS).min(self.env.len());
        if n < 5 * FPS {
            return None;
        }
        // TDF's envelope is 1 - |phase deviation| (rad), whatever the
        // signal's strength, and its seconds are busy with other
        // modulation (no two-level percentiles): fixed, a pulse window's
        // mean (~0.55) low, the plain carrier high
        if self.station == Station::Tdf {
            return Some((0.5, 0.98));
        }
        // two order statistics (no sort: once a second over 30 s)
        let mut v: Vec<f32> = self.env[self.env.len() - n..].to_vec();
        let dn = self.station.down();
        let (il, ih) = (
            ((dn / 2.0) * n as f32) as usize,
            (((dn + (1.0 - dn) / 2.0) * n as f32) as usize).min(n - 1),
        );
        let hi = *v.select_nth_unstable_by(ih, f32::total_cmp).1;
        // the low one lies below
        let lo = if il < ih {
            *v[..ih].select_nth_unstable_by(il, f32::total_cmp).1
        } else {
            hi
        };
        (hi > lo * 1.2).then_some((lo, hi))
    }

    /// The second's phase from the last 20 s, and the match there and at
    /// the current phase.
    fn find_phase(&self, lo: f32, hi: f32) -> Option<(usize, f32, f32, f64)> {
        let n = (20 * FPS).min(self.env.len() / FPS * FPS);
        if n < 8 * FPS {
            return None;
        }
        let start = self.env.len() - n;
        let first_abs = self.env_base + start as u64;
        let mut fold = [0f32; FPS];
        for (i, &v) in self.env[start..].iter().enumerate() {
            let k = ((first_abs + i as u64) % FPS as u64) as usize;
            fold[k] += ((v - lo) / (hi - lo)).clamp(-0.5, 1.5);
        }
        let m = fold.iter().sum::<f32>() / FPS as f32;
        for v in &mut fold {
            *v -= m;
        }
        let tpl = self.station.profile();
        let tm = tpl.iter().sum::<f32>() / FPS as f32;
        let corr =
            |s: usize| -> f32 { (0..FPS).map(|k| fold[(k + s) % FPS] * (tpl[k] - tm)).sum() };
        let mut best = (0, f32::MIN);
        for s in 0..FPS {
            let c = corr(s);
            if c > best.1 {
                best = (s, c);
            }
        }
        let cur = self.phase.map(corr).unwrap_or(0.0);
        // the peak between frames: a parabola through it and its neighbours
        let (cm, cp) = (corr((best.0 + FPS - 1) % FPS), corr((best.0 + 1) % FPS));
        let den = cm - 2.0 * best.1 + cp;
        let frac = if den < 0.0 {
            (0.5 * (cm - cp) / den).clamp(-0.5, 0.5) as f64
        } else {
            0.0
        };
        (best.1 > 0.0).then_some((best.0, best.1, cur, best.0 as f64 + frac))
    }

    fn read(&mut self) {
        let Some((lo, hi)) = self.scale(30) else {
            return;
        };
        self.level = 20.0 * (hi / lo.max(1e-12)).log10();
        // the phase: every 5 s, from 8 s on
        let end_abs = self.env_base + self.env.len() as u64;
        if self.phase.is_none() || end_abs >= self.phase_at + 5 * FPS as u64 {
            self.phase_at = end_abs;
            if let Some((p, c, cur, fine)) = self.find_phase(lo, hi) {
                // a better phase must be clearly better (noise moves the
                // estimate around); a few frames is drift, followed
                let near = self.phase.is_some_and(|q| {
                    (p as i64 - q as i64)
                        .rem_euclid(FPS as i64)
                        .min((q as i64 - p as i64).rem_euclid(FPS as i64))
                        <= 4
                });
                if self.phase != Some(p) && (self.phase.is_none() || near || c > 1.3 * cur) {
                    let shift = self
                        .phase
                        .map(|q| (p as i64 - q as i64).rem_euclid(FPS as i64))
                        .unwrap_or(0);
                    // a small drift moves the next second; a jump restarts
                    if self.phase.is_none() || (shift > 4 && shift < FPS as i64 - 4) {
                        self.next = None;
                        self.syms.clear();
                        self.minute_at = None;
                    }
                    self.phase = Some(p);
                }
                // the fine phase, when it is this peak's (a far one not
                // clearly better is ignored, and so is its fraction)
                if let Some(q) = self.phase {
                    let d = (fine - q as f64 + FPS as f64 / 2.0).rem_euclid(FPS as f64)
                        - FPS as f64 / 2.0;
                    if d.abs() <= 4.5 {
                        self.phase_off = d;
                    }
                }
            }
        }
        let Some(p) = self.phase else { return };
        let next = *self.next.get_or_insert_with(|| {
            let s = self.env_base + (self.env.len() as u64).saturating_sub(10 * FPS as u64);
            s + ((p as u64 + FPS as u64 - s % FPS as u64) % FPS as u64)
        });
        let next = next.max(self.env_base);
        // the second start with this phase nearest the expected one (a
        // phase estimate moving a few frames either way neither skips nor
        // repeats a second)
        let mut at = next - (next % FPS as u64) + p as u64;
        if at + (FPS as u64) / 2 < next {
            at += FPS as u64;
        } else if at > next + (FPS as u64) / 2 && at >= FPS as u64 {
            at -= FPS as u64;
        }
        let at0 = at.max(self.env_base);
        let mut at = at0;
        while at + FPS as u64 + 4 <= end_abs {
            let i0 = (at - self.env_base) as usize;
            let lv = self.window_levels(i0, lo, hi);
            let (_, tpls) = self.station.symbols();
            let mut d: Vec<(f32, char)> = tpls
                .iter()
                .map(|(c, t)| {
                    let d = t.iter().zip(&lv).map(|(a, b)| (a - b).powi(2)).sum::<f32>()
                        / t.len() as f32;
                    (d + self.station.rarity(*c), *c)
                })
                .collect();
            d.sort_by(|a, b| a.0.total_cmp(&b.0));
            let q = (d[1].0.sqrt() - d[0].0.sqrt()).clamp(0.0, 1.0);
            let sym = if q < 0.02 { '?' } else { d[0].1 };
            let t = self.frame_t(at) + self.phase_off * HOP;
            self.second(Second {
                t,
                sym,
                quality: q,
                index: None,
                levels: lv,
            });
            at += FPS as u64;
        }
        self.next = Some(at);
    }

    /// The second starting at frame `i0`: its level in each of the
    /// station's windows, 0 down .. 1 up (the envelope's mean there,
    /// scaled by the levels over the last 30 s). Projecting on the
    /// carrier's phase instead, or scaling by the last few seconds' up and
    /// down windows, read more symbols wrong on a weak DCF77 (2026-11-16:
    /// impulse noise sets the errors, and those references are noisier).
    fn window_levels(&self, i0: usize, lo: f32, hi: f32) -> Vec<f32> {
        let mag = &self.env[i0..i0 + FPS];
        let fr = |t: f64| (t * FPS as f64) as usize;
        let (wins, _) = self.station.symbols();
        wins.iter()
            .map(|&(a, b)| {
                let m = mag[fr(a)..fr(b)].iter().sum::<f32>() / (fr(b) - fr(a)) as f32;
                ((m - lo) / (hi - lo)).clamp(-0.5, 1.5)
            })
            .collect()
    }

    /// A minute's frame decoded: its date seconds voted with the last 10
    /// minutes' (bit errors in the date often pass its one parity bit);
    /// confirmed when its time of day agrees with an earlier minute that
    /// passed, by the time between them.
    fn minute(&mut self, frame: Vec<Second>, next_t: f64) -> Option<Minute> {
        self.frames.push_back(frame.clone());
        while self.frames.len() > 10 {
            self.frames.pop_front();
        }
        let mut voted = frame;
        for r in self.station.date_seconds() {
            for i in r.clone() {
                let mut count: Vec<(char, usize)> = Vec::new();
                for f in &self.frames {
                    let c = f[i].sym;
                    if c == '?' {
                        continue;
                    }
                    match count.iter_mut().find(|x| x.0 == c) {
                        Some(x) => x.1 += 1,
                        None => count.push((c, 1)),
                    }
                }
                if let Some(&(c, k)) = count.iter().max_by_key(|x| x.1)
                    && 2 * k > count.iter().map(|x| x.1).sum::<usize>()
                {
                    voted[i].sym = c;
                }
            }
        }
        let mut m = decode(self.station, &voted, next_t)?;
        let raw: String = self
            .frames
            .back()
            .map(|f| f.iter().map(|s| s.sym).collect())
            .unwrap_or_default();
        m.symbols = raw;
        if m.ok {
            m.confirmed = self.passed.iter().any(|&(t, u)| {
                let k = ((m.t - t) / 60.0).round();
                k >= 1.0 && ((m.t - t) - 60.0 * k).abs() < 0.5 && m.utc - u == 60 * k as i64
            });
            self.passed.push((m.t, m.utc));
            self.passed.retain(|p| m.t - p.0 < 1200.0);
        }
        Some(m)
    }

    fn second(&mut self, mut s: Second) {
        self.syms.push(s.clone());
        let n = self.syms.len();
        match self.minute_at {
            // under way: the next minute starts 60 seconds on, whatever a
            // second in between looked like; two wrong minutes drop it
            Some(m0) if n - 1 - m0 == 60 => {
                let frame: Vec<Second> = self.syms[m0..m0 + 60].to_vec();
                let m = self.minute(frame, self.syms[n - 1].t);
                let good = m.as_ref().is_some_and(|m| m.ok);
                self.bad = if good { 0 } else { self.bad + 1 };
                if let Some(m) = m {
                    self.out.push(Event::Minute(m));
                }
                self.minute_at = (self.bad < 2).then_some(n - 1);
            }
            Some(_) => {}
            // not yet: the latest alignment whose minute passes every check
            None => {
                for st in (0..n.saturating_sub(60)).rev() {
                    let frame = &self.syms[st..st + 60];
                    if decode(self.station, frame, self.syms[st + 60].t).is_some_and(|m| m.ok) {
                        let frame = frame.to_vec();
                        if let Some(m) = self.minute(frame, self.syms[st + 60].t) {
                            self.out.push(Event::Minute(m));
                        }
                        self.minute_at = Some(st + 60);
                        self.bad = 0;
                        break;
                    }
                }
            }
        }
        // keep the current minute (and one before it while searching)
        let keep_from = match self.minute_at {
            Some(m0) => m0,
            None => n.saturating_sub(121),
        };
        if keep_from > 0 {
            self.syms.drain(..keep_from);
            if let Some(m0) = self.minute_at.as_mut() {
                *m0 -= keep_from;
            }
        }
        let n = self.syms.len();
        s.index = self.minute_at.and_then(|m0| u8::try_from(n - 1 - m0).ok());
        self.out.push(Event::Second(s));
    }
}

// ------------------------------------------------------------- decoding

/// Days from 1970-01-01 to a civil date (proleptic Gregorian).
/// Great-circle distance (km) between two points (lat, lon °).
pub fn distance_km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let r = |d: f64| d.to_radians();
    let (la1, lo1, la2, lo2) = (r(a.0), r(a.1), r(b.0), r(b.1));
    let c = la1.sin() * la2.sin() + la1.cos() * la2.cos() * (lo1 - lo2).cos();
    6371.0 * c.clamp(-1.0, 1.0).acos()
}

pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date of a day count from 1970-01-01.
pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn bits_of(frame: &[Second], station: Station) -> Option<Vec<u8>> {
    // DCF77, WWVB, JJY, WWV: one bit a second ('M'/'H' 0 where a marker
    // belongs); MSF: A and B, returned as A in 0..60 and B in 60..120
    let mut a = vec![0u8; 60];
    let mut b = vec![0u8; 60];
    for (i, s) in frame.iter().enumerate().take(60) {
        match (station, s.sym) {
            (Station::Msf, c @ '0'..='3') => {
                let v = c as u8 - b'0';
                a[i] = v & 1;
                b[i] = v >> 1;
            }
            (_, '0') => a[i] = 0,
            (_, '1') => a[i] = 1,
            (_, 'M' | 'H') => {}
            // seconds no field uses (DCF77's and TDF's 1-14: other data,
            // DCF77's encrypted): unread there costs nothing
            (Station::Dcf77 | Station::Tdf, '?') if (1..=14).contains(&i) => {}
            _ => return None,
        }
    }
    if station == Station::Msf {
        a.extend(b);
    }
    Some(a)
}

fn num(b: &[u8], idx: &[(usize, u32)]) -> u32 {
    idx.iter().map(|&(i, w)| u32::from(b[i]) * w).sum()
}

fn parity(b: &[u8], r: std::ops::RangeInclusive<usize>) -> u8 {
    b[r].iter().fold(0, |a, &v| a ^ v)
}

fn markers_ok(frame: &[Second], at: &[usize], sym: char) -> bool {
    at.iter()
        .all(|&i| frame.get(i).is_some_and(|s| s.sym == sym))
}

fn decode(station: Station, frame: &[Second], next_t: f64) -> Option<Minute> {
    let symbols: String = frame.iter().map(|s| s.sym).collect();
    let b = bits_of(frame, station);
    let mut notes = Vec::new();
    let (mut ok, t, y, mo, d, h, mi, zone, zname): (bool, f64, i64, i64, i64, i64, i64, i64, &str);
    match station {
        // their own receivers decode them
        Station::Rbu | Station::Dcf39 | Station::Hga22 | Station::Dcf49 => return None,
        Station::Bpc => {
            // the three frames, each read alone; the minute passes when at
            // least two pass their checks and every one that does agrees
            let frames: Vec<Option<BpcFrame>> = (0..3)
                .map(|f| bpc_frame(&frame[20 * f..20 * f + 20], f))
                .collect();
            let good: Vec<&BpcFrame> = frames.iter().flatten().collect();
            let first = good.first().copied().copied();
            ok = good.len() >= 2 && good.iter().all(|g| Some(**g) == first);
            let g = first.unwrap_or_default();
            t = frame[0].t;
            (y, mo, d, h, mi) = (2000 + g.year, g.month, g.day, g.hour, g.minute);
            (zone, zname) = (station.zone(), "CST");
        }
        Station::Dcf77 | Station::Tdf => {
            let b = b?;
            // the time of the minute that follows
            t = next_t;
            mi = num(
                &b,
                &[
                    (21, 1),
                    (22, 2),
                    (23, 4),
                    (24, 8),
                    (25, 10),
                    (26, 20),
                    (27, 40),
                ],
            ) as i64;
            h = num(
                &b,
                &[(29, 1), (30, 2), (31, 4), (32, 8), (33, 10), (34, 20)],
            ) as i64;
            d = num(
                &b,
                &[(36, 1), (37, 2), (38, 4), (39, 8), (40, 10), (41, 20)],
            ) as i64;
            mo = num(&b, &[(45, 1), (46, 2), (47, 4), (48, 8), (49, 10)]) as i64;
            y = 2000
                + num(
                    &b,
                    &[
                        (50, 1),
                        (51, 2),
                        (52, 4),
                        (53, 8),
                        (54, 10),
                        (55, 20),
                        (56, 40),
                        (57, 80),
                    ],
                ) as i64;
            let cest = b[17] == 1;
            ok = b[0] == 0
                && b[20] == 1
                && (b[17] ^ b[18]) == 1
                && parity(&b, 21..=28) == 0
                && parity(&b, 29..=35) == 0
                && parity(&b, 36..=58) == 0
                && markers_ok(frame, &[59], 'M');
            (zone, zname) = if cest { (7200, "CEST") } else { (3600, "CET") };
            if b[16] == 1 {
                notes.push("summer time change at the hour".into());
            }
            if b[19] == 1 {
                notes.push("leap second at the hour".into());
            }
            if b[15] == 1 {
                notes.push("call bit".into());
            }
        }
        Station::Msf => {
            let v = b?;
            let (a, bb) = v.split_at(60);
            t = next_t;
            y = 2000
                + num(
                    a,
                    &[
                        (17, 80),
                        (18, 40),
                        (19, 20),
                        (20, 10),
                        (21, 8),
                        (22, 4),
                        (23, 2),
                        (24, 1),
                    ],
                ) as i64;
            mo = num(a, &[(25, 10), (26, 8), (27, 4), (28, 2), (29, 1)]) as i64;
            d = num(a, &[(30, 20), (31, 10), (32, 8), (33, 4), (34, 2), (35, 1)]) as i64;
            h = num(a, &[(39, 20), (40, 10), (41, 8), (42, 4), (43, 2), (44, 1)]) as i64;
            mi = num(
                a,
                &[
                    (45, 40),
                    (46, 20),
                    (47, 10),
                    (48, 8),
                    (49, 4),
                    (50, 2),
                    (51, 1),
                ],
            ) as i64;
            let odd = |r: std::ops::RangeInclusive<usize>, p: usize| (parity(a, r) ^ bb[p]) == 1;
            ok = a[52..60] == [0, 1, 1, 1, 1, 1, 1, 0]
                && odd(17..=24, 54)
                && odd(25..=35, 55)
                && odd(36..=38, 56)
                && odd(39..=51, 57)
                && markers_ok(frame, &[0], 'M');
            (zone, zname) = if bb[58] == 1 {
                (3600, "BST")
            } else {
                (0, "GMT")
            };
            if bb[53] == 1 {
                notes.push("summer time change within the hour".into());
            }
            let pos = bb[1..=8].iter().filter(|&&v| v == 1).count();
            let neg = bb[9..=16].iter().filter(|&&v| v == 1).count();
            if pos > 0 || neg > 0 {
                notes.push(format!("DUT1 {:+.1} s", (pos as f64 - neg as f64) / 10.0));
            }
        }
        Station::Wwvb | Station::Jjy => {
            let b = b?;
            t = frame[0].t;
            mi = num(
                &b,
                &[(1, 40), (2, 20), (3, 10), (5, 8), (6, 4), (7, 2), (8, 1)],
            ) as i64;
            h = num(
                &b,
                &[(12, 20), (13, 10), (15, 8), (16, 4), (17, 2), (18, 1)],
            ) as i64;
            let doy = num(
                &b,
                &[
                    (22, 200),
                    (23, 100),
                    (25, 80),
                    (26, 40),
                    (27, 20),
                    (28, 10),
                    (30, 8),
                    (31, 4),
                    (32, 2),
                    (33, 1),
                ],
            ) as i64;
            let marks = [0, 9, 19, 29, 39, 49, 59];
            if station == Station::Wwvb {
                y = 2000
                    + num(
                        &b,
                        &[
                            (45, 80),
                            (46, 40),
                            (47, 20),
                            (48, 10),
                            (50, 8),
                            (51, 4),
                            (52, 2),
                            (53, 1),
                        ],
                    ) as i64;
                let sign = match (b[36], b[37], b[38]) {
                    (1, 0, 1) => Some(1.0),
                    (0, 1, 0) => Some(-1.0),
                    _ => None,
                };
                ok = markers_ok(frame, &marks, 'M') && sign.is_some();
                let dut = num(&b, &[(40, 8), (41, 4), (42, 2), (43, 1)]) as f64 / 10.0;
                if let Some(sg) = sign {
                    notes.push(format!("DUT1 {:+.1} s", sg * dut));
                }
                if b[56] == 1 {
                    notes.push("leap second at the end of the month".into());
                }
                match (b[57], b[58]) {
                    (1, 1) => notes.push("DST in effect".into()),
                    (1, 0) => notes.push("DST begins today".into()),
                    (0, 1) => notes.push("DST ends today".into()),
                    _ => {}
                }
                (zone, zname) = (0, "UTC");
            } else {
                y = 2000
                    + num(
                        &b,
                        &[
                            (41, 80),
                            (42, 40),
                            (43, 20),
                            (44, 10),
                            (45, 8),
                            (46, 4),
                            (47, 2),
                            (48, 1),
                        ],
                    ) as i64;
                ok = markers_ok(frame, &marks, 'M')
                    && parity(&b, 12..=18) == b[36]
                    && parity(&b, 1..=8) == b[37];
                if b[53] == 1 {
                    notes.push(
                        if b[54] == 1 {
                            "leap second (removed) at the end of the month"
                        } else {
                            "leap second at the end of the month"
                        }
                        .into(),
                    );
                }
                (zone, zname) = (station.zone(), "JST");
            }
            let (yy, m2, d2) = civil_from_days(days_from_civil(y, 1, 1) + doy - 1);
            ok &= (1..=366).contains(&doy) && yy == y;
            (mo, d) = (m2, d2);
        }
        Station::Wwv => {
            let b = b?;
            t = frame[0].t;
            mi = num(
                &b,
                &[
                    (10, 1),
                    (11, 2),
                    (12, 4),
                    (13, 8),
                    (15, 10),
                    (16, 20),
                    (17, 40),
                ],
            ) as i64;
            h = num(
                &b,
                &[(20, 1), (21, 2), (22, 4), (23, 8), (25, 10), (26, 20)],
            ) as i64;
            let doy = num(
                &b,
                &[
                    (30, 1),
                    (31, 2),
                    (32, 4),
                    (33, 8),
                    (35, 10),
                    (36, 20),
                    (37, 40),
                    (38, 80),
                    (40, 100),
                    (41, 200),
                ],
            ) as i64;
            y = 2000
                + num(
                    &b,
                    &[
                        (4, 1),
                        (5, 2),
                        (6, 4),
                        (7, 8),
                        (51, 10),
                        (52, 20),
                        (53, 40),
                        (54, 80),
                    ],
                ) as i64;
            ok = markers_ok(frame, &[9, 19, 29, 39, 49, 59], 'M') && markers_ok(frame, &[0], 'H');
            let dut = num(&b, &[(56, 1), (57, 2), (58, 4)]) as f64 / 10.0;
            notes.push(format!(
                "DUT1 {}{:.1} s",
                if b[50] == 1 { '+' } else { '-' },
                dut
            ));
            if b[3] == 1 {
                notes.push("leap second at the end of the month".into());
            }
            if b[55] == 1 || b[2] == 1 {
                notes.push(
                    if b[55] == 1 && b[2] == 1 {
                        "DST in effect"
                    } else {
                        "DST changes today"
                    }
                    .into(),
                );
            }
            let (yy, m2, d2) = civil_from_days(days_from_civil(y, 1, 1) + doy - 1);
            ok &= (1..=366).contains(&doy) && yy == y;
            (mo, d) = (m2, d2);
            (zone, zname) = (0, "UTC");
        }
    }
    ok &= mi < 60 && h < 24 && (1..=12).contains(&mo) && (1..=31).contains(&d);
    let local_s = days_from_civil(y, mo.clamp(1, 12), d.clamp(1, 31)) * 86_400 + h * 3600 + mi * 60;
    let utc = local_s - zone;
    let local = format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02} {zname}");
    Some(Minute {
        station,
        t,
        utc,
        local,
        ok,
        confirmed: false,
        notes,
        symbols,
    })
}

/// One of BPC's 20 s frames, as sent (Beijing time).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct BpcFrame {
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
}

/// BPC's frame `f` (0-2: seconds 0, 20, 40 of the minute) from its 20
/// symbols, when it passes its checks. The layout (a recording through a
/// KiwiSDR at Okayama, 2026-11-22, checked against its GPS time): 0 the
/// marker; 1 the frame's number; 2 zero; 3-4 the hour of 12; 5-7 the
/// minute; 8-9 the weekday (Sunday 0); 10 the afternoon flag (2) and the
/// parity of 1-9's bits (1 when odd); 11-13 the day; 14-15 the month;
/// 16-18 the year in the century; 19 the parity of 11-18's bits. Each
/// symbol is a base-4 digit, most significant first.
fn bpc_frame(s: &[Second], f: usize) -> Option<BpcFrame> {
    if s.len() < 20 || s[0].sym != 'M' {
        return None;
    }
    let v: Vec<u32> = s[1..20]
        .iter()
        .map(|x| x.sym.to_digit(4))
        .collect::<Option<_>>()?;
    // v[k] is second k + 1
    let n = |r: std::ops::Range<usize>| r.fold(0i64, |a, k| a * 4 + i64::from(v[k - 1]));
    let bits =
        |r: std::ops::RangeInclusive<usize>| -> u32 { r.map(|k| v[k - 1].count_ones()).sum() };
    let ok = v[0] as usize == f
        && v[1] == 0
        && v[9] & 1 == bits(1..=9) & 1
        && v[18] & 1 == bits(11..=18) & 1
        // the year's 64 (P4's high bit): not seen yet
        && v[18] >> 1 == 0
        && n(8..10) < 7;
    let hour12 = n(3..5);
    let fr = BpcFrame {
        year: n(16..19),
        month: n(14..16),
        day: n(11..14),
        hour: hour12 + 12 * i64::from(v[9] >> 1),
        minute: n(5..8),
    };
    (ok && hour12 < 12).then_some(fr)
}

// ------------------------------------------------------------- synthesis

/// A station's keying for tests: the level (0..1) of each 1 ms over
/// `minutes` minutes from the UTC `start` (a minute boundary).
pub fn keying(station: Station, start: i64, minutes: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(minutes * 60_000);
    for m in 0..minutes as i64 {
        let utc = start + 60 * m;
        let syms = encode(station, utc);
        for (i, s) in syms.iter().enumerate() {
            let _ = i;
            for ms in 0..1000 {
                let t = ms as f64 / 1000.0;
                out.push(level(station, *s, t));
            }
        }
    }
    out
}

fn level(station: Station, s: char, t: f64) -> f32 {
    let on = match station {
        Station::Rbu | Station::Dcf39 | Station::Hga22 | Station::Dcf49 | Station::Tdf => true,
        Station::Dcf77 => match s {
            '0' => t >= 0.1,
            '1' => t >= 0.2,
            _ => true,
        },
        Station::Msf => {
            let v = s as u8 - b'0';
            match s {
                'M' => t >= 0.5,
                _ => {
                    !(t < 0.1 || (t < 0.2 && v & 1 == 1) || (0.2..0.3).contains(&t) && v >> 1 == 1)
                }
            }
        }
        Station::Bpc => match s {
            c @ '0'..='3' => t >= 0.1 * f64::from(c as u8 - b'0' + 1),
            _ => true,
        },
        Station::Wwvb => {
            t >= match s {
                '0' => 0.2,
                '1' => 0.5,
                _ => 0.8,
            }
        }
        Station::Jjy => {
            t < match s {
                '0' => 0.8,
                '1' => 0.5,
                _ => 0.2,
            }
        }
        Station::Wwv => {
            t >= 0.03
                && t < match s {
                    '0' => 0.2,
                    '1' => 0.5,
                    'M' => 0.8,
                    _ => 0.0,
                }
        }
    };
    if on {
        1.0
    } else if matches!(station, Station::Dcf77) {
        0.15
    } else if matches!(station, Station::Wwvb) {
        0.14
    } else {
        0.0
    }
}

/// The 60 symbols a station sends in the minute starting at `utc`.
pub fn encode(station: Station, utc: i64) -> Vec<char> {
    let mut b = [0u8; 60];
    let mut bb = [0u8; 60];
    let set = |b: &mut [u8; 60], v: i64, idx: &[(usize, i64)]| {
        let mut v = v;
        // weights high to low
        let mut w: Vec<(usize, i64)> = idx.to_vec();
        w.sort_by_key(|x| -x.1);
        for (i, wt) in w {
            if v >= wt {
                b[i] = 1;
                v -= wt;
            }
        }
    };
    let civil = |t: i64| {
        let (y, m, d) = civil_from_days(t.div_euclid(86_400));
        let s = t.rem_euclid(86_400);
        let doy = days_from_civil(y, m, d) - days_from_civil(y, 1, 1) + 1;
        let wd = (t.div_euclid(86_400) + 4).rem_euclid(7); // 0 Sunday
        (y, m, d, s / 3600, s / 60 % 60, doy, wd)
    };
    let mut syms = vec!['0'; 60];
    match station {
        // its own encoder: rbu::encode; EFR: none
        Station::Rbu | Station::Dcf39 | Station::Hga22 | Station::Dcf49 => {}
        Station::Dcf77 | Station::Tdf => {
            // the next minute, CET (no summer time in tests)
            let (y, m, d, h, mi, _, wd) = civil(utc + 60 + 3600);
            b[18] = 1;
            b[20] = 1;
            set(
                &mut b,
                mi,
                &[
                    (21, 1),
                    (22, 2),
                    (23, 4),
                    (24, 8),
                    (25, 10),
                    (26, 20),
                    (27, 40),
                ],
            );
            set(
                &mut b,
                h,
                &[(29, 1), (30, 2), (31, 4), (32, 8), (33, 10), (34, 20)],
            );
            set(
                &mut b,
                d,
                &[(36, 1), (37, 2), (38, 4), (39, 8), (40, 10), (41, 20)],
            );
            set(
                &mut b,
                if wd == 0 { 7 } else { wd },
                &[(42, 1), (43, 2), (44, 4)],
            );
            set(&mut b, m, &[(45, 1), (46, 2), (47, 4), (48, 8), (49, 10)]);
            set(
                &mut b,
                y - 2000,
                &[
                    (50, 1),
                    (51, 2),
                    (52, 4),
                    (53, 8),
                    (54, 10),
                    (55, 20),
                    (56, 40),
                    (57, 80),
                ],
            );
            b[28] = parity(&b, 21..=27);
            b[35] = parity(&b, 29..=34);
            b[58] = parity(&b, 36..=57);
            for i in 0..59 {
                syms[i] = (b'0' + b[i]) as char;
            }
            syms[59] = 'M';
        }
        Station::Msf => {
            let (y, m, d, h, mi, _, wd) = civil(utc + 60);
            set(
                &mut b,
                y - 2000,
                &[
                    (17, 80),
                    (18, 40),
                    (19, 20),
                    (20, 10),
                    (21, 8),
                    (22, 4),
                    (23, 2),
                    (24, 1),
                ],
            );
            set(&mut b, m, &[(25, 10), (26, 8), (27, 4), (28, 2), (29, 1)]);
            set(
                &mut b,
                d,
                &[(30, 20), (31, 10), (32, 8), (33, 4), (34, 2), (35, 1)],
            );
            set(&mut b, wd, &[(36, 4), (37, 2), (38, 1)]);
            set(
                &mut b,
                h,
                &[(39, 20), (40, 10), (41, 8), (42, 4), (43, 2), (44, 1)],
            );
            set(
                &mut b,
                mi,
                &[
                    (45, 40),
                    (46, 20),
                    (47, 10),
                    (48, 8),
                    (49, 4),
                    (50, 2),
                    (51, 1),
                ],
            );
            b[52..60].copy_from_slice(&[0, 1, 1, 1, 1, 1, 1, 0]);
            bb[54] = 1 ^ parity(&b, 17..=24);
            bb[55] = 1 ^ parity(&b, 25..=35);
            bb[56] = 1 ^ parity(&b, 36..=38);
            bb[57] = 1 ^ parity(&b, 39..=51);
            for i in 1..60 {
                syms[i] = (b'0' + b[i] + 2 * bb[i]) as char;
            }
            syms[0] = 'M';
        }
        Station::Wwvb | Station::Jjy => {
            let off = if station == Station::Jjy { 9 * 3600 } else { 0 };
            let (y, _, _, h, mi, doy, wd) = civil(utc + off);
            set(
                &mut b,
                mi,
                &[(1, 40), (2, 20), (3, 10), (5, 8), (6, 4), (7, 2), (8, 1)],
            );
            set(
                &mut b,
                h,
                &[(12, 20), (13, 10), (15, 8), (16, 4), (17, 2), (18, 1)],
            );
            set(
                &mut b,
                doy,
                &[
                    (22, 200),
                    (23, 100),
                    (25, 80),
                    (26, 40),
                    (27, 20),
                    (28, 10),
                    (30, 8),
                    (31, 4),
                    (32, 2),
                    (33, 1),
                ],
            );
            if station == Station::Wwvb {
                b[36] = 1;
                b[38] = 1; // DUT1 +
                set(&mut b, 3, &[(40, 8), (41, 4), (42, 2), (43, 1)]);
                set(
                    &mut b,
                    y - 2000,
                    &[
                        (45, 80),
                        (46, 40),
                        (47, 20),
                        (48, 10),
                        (50, 8),
                        (51, 4),
                        (52, 2),
                        (53, 1),
                    ],
                );
            } else {
                b[36] = parity(&b, 12..=18);
                b[37] = parity(&b, 1..=8);
                set(
                    &mut b,
                    y - 2000,
                    &[
                        (41, 80),
                        (42, 40),
                        (43, 20),
                        (44, 10),
                        (45, 8),
                        (46, 4),
                        (47, 2),
                        (48, 1),
                    ],
                );
                set(&mut b, wd, &[(50, 4), (51, 2), (52, 1)]);
            }
            for i in 0..60 {
                syms[i] = (b'0' + b[i]) as char;
            }
            for i in [0, 9, 19, 29, 39, 49, 59] {
                syms[i] = 'M';
            }
        }
        Station::Bpc => {
            let (y, m, d, h, mi, _, wd) = civil(utc + station.zone());
            let digits = |v: i64, n: usize| -> Vec<u32> {
                (0..n).rev().map(|k| ((v >> (2 * k)) & 3) as u32).collect()
            };
            for f in 0..3 {
                let mut v = vec![f as u32, 0];
                v.extend(digits(h % 12, 2));
                v.extend(digits(mi, 3));
                v.extend(digits(wd, 2));
                let p1: u32 = v.iter().map(|x| x.count_ones()).sum::<u32>() & 1;
                v.push(2 * u32::from(h >= 12) + p1);
                let mut dv = digits(d, 3);
                dv.extend(digits(m, 2));
                dv.extend(digits(y - 2000, 3));
                let p2: u32 = dv.iter().map(|x| x.count_ones()).sum::<u32>() & 1;
                v.extend(dv);
                v.push(p2);
                syms[20 * f] = 'M';
                for (k, x) in v.iter().enumerate() {
                    syms[20 * f + 1 + k] = char::from(b'0' + *x as u8);
                }
            }
        }
        Station::Wwv => {
            let (y, _, _, h, mi, doy, _) = civil(utc);
            set(&mut b, (y - 2000) % 10, &[(4, 1), (5, 2), (6, 4), (7, 8)]);
            set(
                &mut b,
                (y - 2000) / 10 * 10,
                &[(51, 10), (52, 20), (53, 40), (54, 80)],
            );
            set(
                &mut b,
                mi,
                &[
                    (10, 1),
                    (11, 2),
                    (12, 4),
                    (13, 8),
                    (15, 10),
                    (16, 20),
                    (17, 40),
                ],
            );
            set(
                &mut b,
                h,
                &[(20, 1), (21, 2), (22, 4), (23, 8), (25, 10), (26, 20)],
            );
            set(
                &mut b,
                doy,
                &[
                    (30, 1),
                    (31, 2),
                    (32, 4),
                    (33, 8),
                    (35, 10),
                    (36, 20),
                    (37, 40),
                    (38, 80),
                    (40, 100),
                    (41, 200),
                ],
            );
            b[50] = 1;
            set(&mut b, 2, &[(56, 1), (57, 2), (58, 4)]);
            for i in 0..60 {
                syms[i] = (b'0' + b[i]) as char;
            }
            for i in [9, 19, 29, 39, 49, 59] {
                syms[i] = 'M';
            }
            syms[0] = 'H';
        }
    }
    syms
}
