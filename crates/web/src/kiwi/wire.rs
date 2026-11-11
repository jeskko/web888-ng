// SPDX-License-Identifier: GPL-3.0-or-later
//! The Kiwi protocol on the wire (`doc/m5-kiwi.md` §2): the server's
//! binary frames (`MSG`, `SND`, `W/F `), its URL-encoded values, and the
//! client's `SET` commands.
//!
//! The layouts follow the upstream server (KiwiSDR-derived, RaspSDR/server:
//! `rx/rx_sound.cpp`, `rx/rx_waterfall.cpp`, `rx/rx_cmd.cpp`, LGPL-2.0+,
//! Copyright (c) 2014-2023 John Seamons, ZL4VO/KF6VO) and what kiwiclient
//! reads; the code is written for web888d.

use super::adpcm::Adpcm;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

/// `SND` header flags.
pub const SND_FLAG_LPF: u8 = 0x01;
pub const SND_FLAG_ADC_OVFL: u8 = 0x02;
pub const SND_FLAG_NEW_FREQ: u8 = 0x04;
pub const SND_FLAG_STEREO: u8 = 0x08;
pub const SND_FLAG_COMPRESSED: u8 = 0x10;
pub const SND_FLAG_RESTART: u8 = 0x20;
pub const SND_FLAG_SQUELCH_UI: u8 = 0x40;
pub const SND_FLAG_LITTLE_ENDIAN: u8 = 0x80;

/// `SND` header: "SND", flags, sequence (LE u32), S-meter (BE u16).
pub const SND_HEADER: usize = 10;
/// Stereo (IQ) packets add the GPS time: last solution, a pad byte, GPS
/// seconds and nanoseconds (LE u32).
pub const SND_GPS_HEADER: usize = 10;
/// The S-meter's bias: the value is (dBm + 127) * 10, as KiwiSDR and
/// kiwiclient have it (the Web-888 fork moved it to 140).
pub const SMETER_BIAS: f32 = 127.0;

/// Samples (per channel) in a packet: 1024 bytes of mono PCM16, of
/// ADPCM (four times the samples), or 2048 bytes of I/Q.
pub fn snd_samples(channels: usize, compressed: bool) -> usize {
    if channels == 1 && compressed {
        2048
    } else {
        512
    }
}

/// `W/F ` header: tag (4 bytes), first bin (LE u32), zoom and flags (LE
/// u32), sequence (LE u32).
pub const WF_HEADER: usize = 16;
pub const WF_FLAGS_COMPRESSION: u32 = 0x0001_0000;
/// Pixels in a waterfall line.
pub const WF_WIDTH: usize = 1024;
/// Zoom levels 0..=MAX_ZOOM; the start bin counts in units of the
/// deepest zoom's pixels (span / (1024 << MAX_ZOOM)).
pub const MAX_ZOOM: u32 = 14;
/// Waterfall rates for `SET wf_speed=0..4` (frames/s; 0: off).
pub const WF_SPEEDS: [u32; 5] = [0, 1, 5, 13, 23];
/// Extra ADPCM samples ahead of a compressed line (the first level
/// repeated) so the decoder has adapted when the line starts.
pub const ADPCM_PAD: usize = 10;

/// A `MSG` frame: "MSG " and the text.
pub fn msg(text: &str) -> Vec<u8> {
    let mut b = Vec::with_capacity(4 + text.len());
    b.extend_from_slice(b"MSG ");
    b.extend_from_slice(text.as_bytes());
    b
}

/// Percent-encode everything but unreserved characters (`encodeURIComponent`
/// and Python's `unquote` read it back).
pub fn url_encode(s: &str) -> String {
    let mut o = String::with_capacity(s.len() * 3 / 2);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

/// Undo `%xx` escapes (invalid ones are kept as they are); `+` stays.
pub fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut o = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            o.push((h * 16 + l) as u8);
            i += 3;
        } else {
            o.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&o).into_owned()
}

/// A command from a Kiwi client. What web888d doesn't do is `Other`
/// (accepted and ignored, as the upstream server ignores what it lacks).
#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    /// `SET auth t=kiwi|admin p=<password> [ipl=...]`; "#" is no password.
    Auth {
        admin: bool,
        password: String,
    },
    Keepalive,
    /// `SET ident_user=<name>` (URL-encoded or plain).
    Ident(String),
    /// `SET mod=<mode> low_cut= high_cut= freq=<kHz>`: mode "x" keeps the
    /// mode, a 0..0 passband keeps the passband.
    Tune {
        mode: Option<String>,
        lo: f64,
        hi: f64,
        khz: f64,
    },
    /// `SET agc= hang= thresh= slope= decay= manGain=`.
    Agc {
        on: bool,
        hang: bool,
        decay_ms: i32,
        gain_db: i32,
    },
    Compression(bool),
    LittleEndian,
    Reinit,
    /// `SET AR OK in= out=`: the client's audio is set up.
    ArOk,
    /// `SET nb type=0 en=` (the blanker; other types are ignored) or the
    /// old `SET nb=<gate> th=<thresh>`.
    Nb(bool),
    /// `SET zoom=<z> cf=<kHz>` or `SET zoom=<z> start=<bins>`.
    Zoom {
        zoom: u32,
        cf_khz: Option<f64>,
        start: Option<f64>,
    },
    /// `SET wf_speed=<0..4>` (-1: fastest).
    WfSpeed(i32),
    WfComp(bool),
    Other,
}

/// `k=v` pairs (and bare words, value "").
fn pairs(s: &str) -> impl Iterator<Item = (&str, &str)> {
    s.split_ascii_whitespace()
        .map(|t| t.split_once('=').unwrap_or((t, "")))
}

fn get<'a>(s: &'a str, k: &str) -> Option<&'a str> {
    pairs(s).find(|&(a, _)| a == k).map(|(_, v)| v)
}

fn num<T: std::str::FromStr>(s: &str, k: &str) -> Option<T> {
    get(s, k)?.parse().ok()
}

impl Cmd {
    pub fn parse(line: &str) -> Cmd {
        let line = line.trim_end_matches(['\n', '\r', '\0']);
        let Some(s) = line.strip_prefix("SET ") else {
            return Cmd::Other;
        };
        let first = s.split_ascii_whitespace().next().unwrap_or("");
        let key = first.split_once('=').map_or(first, |(k, _)| k);
        let parsed = match key {
            "auth" => get(s, "t").map(|t| Cmd::Auth {
                admin: t == "admin",
                password: match get(s, "p") {
                    None | Some("#") => String::new(),
                    Some(p) => url_decode(p),
                },
            }),
            "keepalive" => Some(Cmd::Keepalive),
            "ident_user" => Some(Cmd::Ident(url_decode(
                s.strip_prefix("ident_user=").unwrap_or(""),
            ))),
            "mod" => (|| {
                let m = get(s, "mod")?;
                Some(Cmd::Tune {
                    mode: (m != "x").then(|| m.to_ascii_lowercase()),
                    lo: num(s, "low_cut")?,
                    hi: num(s, "high_cut")?,
                    khz: num(s, "freq")?,
                })
            })(),
            "agc" => (|| {
                Some(Cmd::Agc {
                    on: num::<i32>(s, "agc")? != 0,
                    hang: num::<i32>(s, "hang").unwrap_or(0) != 0,
                    decay_ms: num(s, "decay").unwrap_or(1000),
                    gain_db: num(s, "manGain").unwrap_or(50),
                })
            })(),
            "compression" => num::<i32>(s, "compression").map(|c| Cmd::Compression(c != 0)),
            "little-endian" => Some(Cmd::LittleEndian),
            "reinit" => Some(Cmd::Reinit),
            "AR" => (s.starts_with("AR OK") && num::<i32>(s, "out").is_some_and(|o| o != 0))
                .then_some(Cmd::ArOk),
            "nb" if get(s, "th").is_some() => num::<i32>(s, "nb").map(|g| Cmd::Nb(g != 0)),
            "nb" => match (num::<i32>(s, "type"), num::<i32>(s, "en")) {
                (Some(0), Some(en)) => Some(Cmd::Nb(en != 0)),
                _ => None,
            },
            "zoom" => (|| {
                Some(Cmd::Zoom {
                    zoom: num(s, "zoom")?,
                    cf_khz: num(s, "cf"),
                    start: num(s, "start"),
                })
                .filter(|c| {
                    matches!(
                        c,
                        Cmd::Zoom {
                            cf_khz: Some(_),
                            ..
                        } | Cmd::Zoom { start: Some(_), .. }
                    )
                })
            })(),
            "wf_speed" => num(s, "wf_speed").map(Cmd::WfSpeed),
            "wf_comp" => num::<i32>(s, "wf_comp").map(|c| Cmd::WfComp(c != 0)),
            _ => None,
        };
        parsed.unwrap_or(Cmd::Other)
    }
}

fn pcm16(x: f32) -> i16 {
    (x * 32767.0).round().clamp(-32768.0, 32767.0) as i16
}

/// One SND stream's packets: compression, byte order, the ADPCM state and
/// the sequence (shared with the stream's waterfall, which tags its lines
/// with it).
pub struct SndEnc {
    pub compression: bool,
    pub little_endian: bool,
    /// dBm = dBFS + this (`[kiwi] smeter_offset_db`).
    pub dbm_offset: f32,
    pub seq: Arc<AtomicU32>,
    adpcm: Adpcm,
    /// Flags for the next packet (new frequency, passband, restart).
    pending: u8,
    channels: usize,
}

/// Changes to a stream's packets.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SndSet {
    pub compression: Option<bool>,
    pub little_endian: bool,
    pub dbm_offset: Option<f32>,
    /// Flags for the next packet.
    pub flags: u8,
}

impl SndEnc {
    pub fn new(seq: Arc<AtomicU32>, dbm_offset: f32) -> Self {
        SndEnc {
            // the upstream default; kiwirecorder turns it off
            compression: true,
            little_endian: false,
            dbm_offset,
            seq,
            adpcm: Adpcm::default(),
            pending: 0,
            channels: 1,
        }
    }

    pub fn set(&mut self, s: SndSet) {
        if let Some(c) = s.compression {
            if c && !self.compression {
                self.adpcm = Adpcm::default();
            }
            self.compression = c;
        }
        self.little_endian |= s.little_endian;
        if let Some(d) = s.dbm_offset {
            self.dbm_offset = d;
        }
        if s.flags & SND_FLAG_RESTART != 0 {
            self.adpcm = Adpcm::default();
        }
        self.pending |= s.flags;
    }

    /// Samples per channel in the next packet for a chain of `channels`.
    pub fn samples(&self, channels: usize) -> usize {
        snd_samples(channels, self.compression)
    }

    /// One packet of `pcm` (`channels` interleaved, full scale 1.0).
    pub fn encode(
        &mut self,
        overload: bool,
        squelched: bool,
        rssi_dbfs: f32,
        pcm: &[f32],
        channels: usize,
    ) -> Vec<u8> {
        if channels != self.channels {
            // the client starts its decoder over on a mono/stereo change
            self.adpcm = Adpcm::default();
            self.channels = channels;
        }
        let stereo = channels == 2;
        let comp = self.compression && !stereo;
        let mut flags = std::mem::take(&mut self.pending);
        if overload {
            flags |= SND_FLAG_ADC_OVFL;
        }
        if stereo {
            flags |= SND_FLAG_STEREO;
        }
        if comp {
            flags |= SND_FLAG_COMPRESSED;
        }
        if squelched {
            flags |= SND_FLAG_SQUELCH_UI;
        }
        if self.little_endian && !comp {
            flags |= SND_FLAG_LITTLE_ENDIAN;
        }
        let seq = self.seq.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let dbm = (rssi_dbfs + self.dbm_offset).clamp(-SMETER_BIAS, 3.4);
        let smeter = ((dbm + SMETER_BIAS) * 10.0) as u16;
        let mut b = Vec::with_capacity(SND_HEADER + SND_GPS_HEADER + 2 * pcm.len());
        b.extend_from_slice(b"SND");
        b.push(flags);
        b.extend_from_slice(&seq.to_le_bytes());
        b.extend_from_slice(&smeter.to_be_bytes());
        if stereo {
            // no GPS time (web888d has no timestamps for it yet)
            b.extend_from_slice(&[0; SND_GPS_HEADER]);
        }
        if comp {
            let x: Vec<i16> = pcm.iter().map(|&v| pcm16(v)).collect();
            self.adpcm.encode_i16(&x, &mut b);
        } else {
            for &v in pcm {
                let s = pcm16(v);
                b.extend_from_slice(&if self.little_endian {
                    s.to_le_bytes()
                } else {
                    s.to_be_bytes()
                });
            }
        }
        b
    }
}

/// A waterfall level byte: 0..-200 dBm as 255..55 (the upstream server's
/// mapping: `(u8)(int)(dBm - 1)`).
pub fn wf_byte(dbm: f32) -> u8 {
    let d = if dbm.is_finite() {
        dbm.clamp(-200.0, 0.0)
    } else {
        -200.0
    };
    ((d - 1.0) as i32) as u8
}

/// What a W/F stream shows: its zoom and start, the RF span they mean,
/// and how lines go out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WfView {
    /// RF (Hz) at the line's left edge, and its width.
    pub start_hz: f64,
    pub span_hz: f64,
    /// The start in the protocol's bins, and the zoom.
    pub x_bin: u32,
    pub zoom: u32,
    pub compression: bool,
    pub dbm_offset: f32,
}

/// Kiwi waterfall lines from the server's levels (any bins, any span):
/// resampled to 1024 pixels of the view.
pub struct WfEnc {
    pub view: Mutex<WfView>,
    /// The sequence of the paired SND stream (or the stream's own count).
    pub seq: Arc<AtomicU32>,
    /// Count lines into `seq` (no SND stream does).
    pub own_seq: bool,
}

impl WfEnc {
    /// A line of levels `db` (dBFS) for bins centred at `f0 + k * df` (Hz).
    pub fn encode(&self, f0: f64, df: f64, db: &[f32]) -> Vec<u8> {
        let v = *self.view.lock().unwrap();
        let px = v.span_hz / WF_WIDTH as f64;
        let mut levels = [0u8; WF_WIDTH];
        let n = db.len() as i64;
        for (i, l) in levels.iter_mut().enumerate() {
            let lo = v.start_hz + i as f64 * px;
            // bins whose centres fall in the pixel; else the nearest
            let k0 = ((lo - f0) / df).ceil() as i64;
            let k1 = ((lo + px - f0) / df).ceil() as i64;
            let (k0, k1) = if k1 > k0 {
                (k0, k1)
            } else {
                let k = ((lo + px / 2.0 - f0) / df).round() as i64;
                (k, k + 1)
            };
            let (k0, k1) = (k0.clamp(0, n), k1.clamp(0, n));
            let peak = db[k0 as usize..k1 as usize]
                .iter()
                .copied()
                .fold(f32::NEG_INFINITY, f32::max);
            *l = wf_byte(peak + v.dbm_offset);
        }
        let seq = if self.own_seq {
            self.seq.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
        } else {
            self.seq.load(Ordering::Relaxed)
        };
        let mut b = Vec::with_capacity(WF_HEADER + WF_WIDTH);
        b.extend_from_slice(b"W/F ");
        b.extend_from_slice(&v.x_bin.to_le_bytes());
        let flags = v.zoom
            | if v.compression {
                WF_FLAGS_COMPRESSION
            } else {
                0
            };
        b.extend_from_slice(&flags.to_le_bytes());
        b.extend_from_slice(&seq.to_le_bytes());
        if v.compression {
            let mut x = Vec::with_capacity(ADPCM_PAD + WF_WIDTH);
            x.extend(std::iter::repeat_n(levels[0], ADPCM_PAD));
            x.extend_from_slice(&levels);
            Adpcm::default().encode_u8(&x, &mut b);
        } else {
            b.extend_from_slice(&levels);
        }
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_parse() {
        use Cmd::*;
        assert_eq!(
            Cmd::parse("SET auth t=kiwi p=#"),
            Auth {
                admin: false,
                password: String::new()
            }
        );
        assert_eq!(
            Cmd::parse("SET auth t=admin p=se%20cret ipl=x"),
            Auth {
                admin: true,
                password: "se cret".into()
            }
        );
        assert_eq!(
            Cmd::parse("SET auth t=kiwi p="),
            Auth {
                admin: false,
                password: String::new()
            }
        );
        assert_eq!(
            Cmd::parse("SET mod=usb low_cut=300 high_cut=2700 freq=7074.000"),
            Tune {
                mode: Some("usb".into()),
                lo: 300.0,
                hi: 2700.0,
                khz: 7074.0
            }
        );
        assert_eq!(
            Cmd::parse("SET mod=x low_cut=0 high_cut=0 freq=14074.5"),
            Tune {
                mode: None,
                lo: 0.0,
                hi: 0.0,
                khz: 14074.5
            }
        );
        assert_eq!(
            Cmd::parse("SET agc=0 hang=0 thresh=-100 slope=6 decay=1000 manGain=50"),
            Agc {
                on: false,
                hang: false,
                decay_ms: 1000,
                gain_db: 50
            }
        );
        assert_eq!(Cmd::parse("SET AR OK in=12000 out=44100"), ArOk);
        assert_eq!(Cmd::parse("SET AR OK in=12000 out=0"), Other);
        assert_eq!(Cmd::parse("SET compression=0"), Compression(false));
        assert_eq!(Cmd::parse("SET keepalive"), Keepalive);
        assert_eq!(
            Cmd::parse("SET ident_user=N0CALL%20test"),
            Ident("N0CALL test".into())
        );
        assert_eq!(Cmd::parse("SET ident_user=a b"), Ident("a b".into()));
        assert_eq!(Cmd::parse("SET nb type=0 en=1"), Nb(true));
        assert_eq!(Cmd::parse("SET nb type=2 en=1"), Other);
        assert_eq!(Cmd::parse("SET nb=100 th=50"), Nb(true));
        assert_eq!(
            Cmd::parse("SET zoom=3 cf=7100.000000"),
            Zoom {
                zoom: 3,
                cf_khz: Some(7100.0),
                start: None
            }
        );
        assert_eq!(
            Cmd::parse("SET zoom=0 start=0"),
            Zoom {
                zoom: 0,
                cf_khz: None,
                start: Some(0.0)
            }
        );
        assert_eq!(Cmd::parse("SET zoom=3"), Other);
        assert_eq!(Cmd::parse("SET wf_speed=-1"), WfSpeed(-1));
        assert_eq!(Cmd::parse("SET wf_comp=1"), WfComp(true));
        assert_eq!(Cmd::parse("SET genattn=0"), Other);
        assert_eq!(Cmd::parse("SET mod=usb"), Other, "incomplete");
        assert_eq!(Cmd::parse("hello"), Other);
    }

    #[test]
    fn url_coding_round_trips() {
        let s = r#"{"rx_gps":"(60.5, 25.0)","n":"ä &x"}"#;
        let e = url_encode(s);
        assert!(!e.contains(['"', ' ', '{', '&']));
        assert_eq!(url_decode(&e), s);
        assert_eq!(url_decode("100%"), "100%");
        assert_eq!(url_decode("%zz"), "%zz");
    }

    #[test]
    fn snd_packets_have_the_documented_layout() {
        let seq = Arc::new(AtomicU32::new(0));
        let mut e = SndEnc::new(seq.clone(), 0.0);
        e.set(SndSet {
            compression: Some(false),
            flags: SND_FLAG_NEW_FREQ,
            ..Default::default()
        });
        let p = e.encode(true, false, -73.0, &[0.5, -1.0], 1);
        assert_eq!(&p[..3], b"SND");
        assert_eq!(p[3], SND_FLAG_NEW_FREQ | SND_FLAG_ADC_OVFL);
        assert_eq!(u32::from_le_bytes(p[4..8].try_into().unwrap()), 1);
        // kiwiclient: 0.1 * smeter - 127
        let sm = u16::from_be_bytes([p[8], p[9]]);
        assert!((0.1 * sm as f32 - 127.0 + 73.0).abs() < 0.11, "{sm}");
        assert_eq!(i16::from_be_bytes([p[10], p[11]]), 16384);
        assert_eq!(i16::from_be_bytes([p[12], p[13]]), -32767);
        assert_eq!(seq.load(Ordering::Relaxed), 1);
        // the flag went with one packet
        let p = e.encode(false, false, -73.0, &[0.0; 2], 1);
        assert_eq!(p[3], 0);

        // stereo: the GPS header, never compressed
        e.set(SndSet {
            compression: Some(true),
            ..Default::default()
        });
        let p = e.encode(false, false, -73.0, &[0.25, -0.25], 2);
        assert_eq!(p[3], SND_FLAG_STEREO);
        assert_eq!(p.len(), SND_HEADER + SND_GPS_HEADER + 4);
        assert_eq!(i16::from_be_bytes([p[20], p[21]]), 8192);

        // compressed mono: 4 bits a sample
        let p = e.encode(false, true, -200.0, &[0.0; 2048], 1);
        assert_eq!(p[3], SND_FLAG_COMPRESSED | SND_FLAG_SQUELCH_UI);
        assert_eq!(p.len(), SND_HEADER + 1024);
        assert_eq!(u16::from_be_bytes([p[8], p[9]]), 0, "clamped at -127 dBm");
        assert_eq!(snd_samples(1, true), 2048);
        assert_eq!(snd_samples(2, true), 512);
    }

    #[test]
    fn waterfall_bytes_and_lines() {
        assert_eq!(wf_byte(0.0), 255);
        assert_eq!(wf_byte(-200.0), 55);
        assert_eq!(wf_byte(-100.0), 155);
        assert_eq!(wf_byte(f32::NAN), 55);

        let e = WfEnc {
            view: Mutex::new(WfView {
                start_hz: 1000.0,
                span_hz: 1024.0,
                x_bin: 77,
                zoom: 5,
                compression: false,
                dbm_offset: -10.0,
            }),
            seq: Arc::new(AtomicU32::new(41)),
            own_seq: false,
        };
        // 0.5 Hz bins from 500 Hz: two a pixel, the stronger shown
        let db: Vec<f32> = (0..4000)
            .map(|k| if k == 1001 { -20.0 } else { -90.0 })
            .collect();
        let l = e.encode(500.0, 0.5, &db);
        assert_eq!(&l[..4], b"W/F ");
        assert_eq!(u32::from_le_bytes(l[4..8].try_into().unwrap()), 77);
        assert_eq!(u32::from_le_bytes(l[8..12].try_into().unwrap()), 5);
        assert_eq!(u32::from_le_bytes(l[12..16].try_into().unwrap()), 41);
        let px = &l[WF_HEADER..];
        assert_eq!(px.len(), WF_WIDTH);
        // bin 1001 is at 1000.5 Hz: pixel 0
        assert_eq!(px[0], wf_byte(-30.0));
        assert_eq!(px[1], wf_byte(-100.0));
        // 4 Hz bins: each pixel the nearest one; past the data: the floor
        let l = e.encode(1000.0, 4.0, &[-50.0; 200]);
        assert_eq!(l[WF_HEADER + 10], wf_byte(-60.0));
        assert_eq!(l[WF_HEADER + 1000], wf_byte(-200.0));

        e.view.lock().unwrap().compression = true;
        let l = e.encode(1000.0, 4.0, &[-50.0; 300]);
        assert_eq!(
            u32::from_le_bytes(l[8..12].try_into().unwrap()),
            5 | WF_FLAGS_COMPRESSION
        );
        assert_eq!(l.len(), WF_HEADER + (ADPCM_PAD + WF_WIDTH) / 2);
    }
}
