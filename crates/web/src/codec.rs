// SPDX-License-Identifier: GPL-3.0-or-later
//! The web UI's audio codecs (`doc/m3-multi-user.md` §8): PCM16 (as
//! before), G.711 mu-law (2:1; the browser decoders don't notice it) and
//! Opus (`opus1`, a Rust port of libopus 1.6; 24-48 kbit/s, for
//! listening). The client asks for one with `audio`'s `codec`; each frame
//! says which it carries (`FLAG_MULAW`, `FLAG_OPUS`), so a change takes
//! effect at a frame boundary.
//!
//! Opus is told the audio's bandwidth from the passband: narrowband
//! (4 kHz) when the audio stays under 4 kHz (SSB, CW, the digital modes),
//! where it spends its bits on the signal; left to itself it codes 12 kS/s
//! as wideband and the FT8 decoder loses a quarter of its decodes at
//! 24 kbit/s instead of a tenth (`/data/iq/codecs`). Complexity 0: ~2 % of
//! an A9 core a stream, no worse for decoding than higher ones.

use crate::proto::{AUDIO_HEADER, FLAG_MULAW, FLAG_OPUS, KIND_AUDIO};
use std::num::NonZeroU32;
use web888_dsp::audio::{AudioSpec, Mode};

/// What a session's audio is sent as.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Codec {
    #[default]
    Pcm16,
    Mulaw,
    Opus,
}

impl Codec {
    /// The codecs the server sends, by their protocol names.
    pub const ALL: [Codec; 3] = [Codec::Pcm16, Codec::Mulaw, Codec::Opus];

    pub fn name(self) -> &'static str {
        match self {
            Codec::Pcm16 => "pcm16",
            Codec::Mulaw => "mulaw",
            Codec::Opus => "opus",
        }
    }

    pub fn parse(s: &str) -> Option<Codec> {
        Codec::ALL.into_iter().find(|c| c.name() == s)
    }
}

/// G.711 mu-law of a 16-bit sample.
pub fn mulaw(s: i16) -> u8 {
    const BIAS: i32 = 0x84;
    let mut x = s as i32;
    let sign = if x < 0 {
        x = -x;
        0x80
    } else {
        0
    };
    x = x.min(32635) + BIAS;
    let exp = (31 - (x as u32).leading_zeros() as i32 - 7).clamp(0, 7);
    let mant = (x >> (exp + 3)) & 0x0f;
    !(sign | (exp << 4) | mant) as u8
}

/// The sample a mu-law code stands for (the UI's table is this).
pub fn mulaw_decode(u: u8) -> i16 {
    let u = !u as i32;
    let exp = (u >> 4) & 7;
    let x = ((((u & 0x0f) << 3) + 0x84) << exp) - 0x84;
    (if u & 0x80 != 0 { -x } else { x }) as i16
}

fn to_i16(s: f32) -> i16 {
    (s * 32767.0).round().clamp(-32768.0, 32767.0) as i16
}

/// The highest audio frequency `spec` gives (Hz).
fn audio_top(spec: &AudioSpec) -> f64 {
    match spec.mode {
        Mode::Cw => spec.pitch_hz + 0.5 * (spec.hi_hz - spec.lo_hz),
        _ => spec.lo_hz.abs().max(spec.hi_hz.abs()),
    }
}

/// The Opus bandwidth for audio up to `top` Hz at `rate`, and its bitrate
/// (bit/s, mono).
fn opus_band(top: f64, rate: u32) -> (opus1::Bandwidth, u32) {
    if top <= 4000.0 {
        (opus1::Bandwidth::Narrowband, 24_000)
    } else if top <= 8000.0 || rate <= 16_000 {
        (opus1::Bandwidth::Wideband, 32_000)
    } else {
        (opus1::Bandwidth::Superwideband, 48_000)
    }
}

struct OpusEnc {
    enc: Box<opus1::OpusEncoder>,
    rate: u32,
    chans: usize,
    band: Option<(opus1::Bandwidth, u32)>,
}

impl OpusEnc {
    fn new(rate: u32, chans: usize) -> Option<Self> {
        let sr = opus1::SampleRate::try_from(rate as i32).ok()?;
        let ch = if chans == 2 {
            opus1::Channels::Stereo
        } else {
            opus1::Channels::Mono
        };
        let mut enc = opus1::OpusEncoder::new(sr, ch, opus1::Application::Audio).ok()?;
        enc.set_complexity(0).ok()?;
        Some(OpusEnc {
            enc: Box::new(enc),
            rate,
            chans,
            band: None,
        })
    }

    fn set_band(&mut self, band: (opus1::Bandwidth, u32)) {
        if self.band == Some(band) {
            return;
        }
        self.band = Some(band);
        self.enc.set_bandwidth(Some(band.0));
        // stereo: half as much again
        let bps = band.1 * if self.chans == 2 { 3 } else { 2 } / 2;
        self.enc
            .set_bitrate(opus1::Bitrate::BitsPerSecond(NonZeroU32::new(bps).unwrap()));
    }
}

/// A session's audio encoder: frames in the codec asked for.
#[derive(Default)]
pub struct WebEnc {
    codec: Codec,
    opus: Option<OpusEnc>,
    pcm: Vec<i16>,
    packet: Vec<u8>,
}

impl WebEnc {
    pub fn set_codec(&mut self, c: Codec) {
        if c != self.codec {
            self.codec = c;
            self.opus = None;
        }
    }

    pub fn codec(&self) -> Codec {
        self.codec
    }

    /// One frame of `pcm` (20 ms; `chans` interleaved) at `rate`: the
    /// header (`flags` gains the codec's) and the payload. `spec` sets
    /// Opus's bandwidth. Falls back to PCM16 where Opus can't take the
    /// frame (a rate it lacks, a frame length it lacks).
    #[allow(clippy::too_many_arguments)]
    pub fn frame(
        &mut self,
        flags: u8,
        seq: u32,
        rate: u32,
        rssi_db: f32,
        pcm: &[f32],
        chans: usize,
        spec: &AudioSpec,
    ) -> Vec<u8> {
        self.pcm.clear();
        self.pcm.extend(pcm.iter().map(|&s| to_i16(s)));
        let (flag, payload): (u8, &[u8]) = match self.codec {
            Codec::Pcm16 => return crate::proto::audio_frame(flags, seq, rate, rssi_db, pcm),
            Codec::Mulaw => {
                self.packet.clear();
                self.packet.extend(self.pcm.iter().map(|&s| mulaw(s)));
                (FLAG_MULAW, &self.packet)
            }
            Codec::Opus => {
                if self
                    .opus
                    .as_ref()
                    .is_none_or(|o| o.rate != rate || o.chans != chans)
                {
                    self.opus = OpusEnc::new(rate, chans);
                }
                let Some(o) = &mut self.opus else {
                    return crate::proto::audio_frame(flags, seq, rate, rssi_db, pcm);
                };
                o.set_band(opus_band(audio_top(spec), rate));
                self.packet.resize(1500, 0);
                let per_chan = self.pcm.len() / chans.max(1);
                match o.enc.encode_i16(&self.pcm, per_chan, &mut self.packet) {
                    Ok(n) => {
                        self.packet.truncate(n);
                        (FLAG_OPUS, &self.packet)
                    }
                    Err(_) => return crate::proto::audio_frame(flags, seq, rate, rssi_db, pcm),
                }
            }
        };
        let mut b = Vec::with_capacity(AUDIO_HEADER + payload.len());
        b.push(KIND_AUDIO);
        b.push(flags | flag);
        // the samples the frame decodes to (both channels')
        b.extend_from_slice(&(pcm.len() as u16).to_le_bytes());
        b.extend_from_slice(&seq.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&rssi_db.to_le_bytes());
        b.extend_from_slice(payload);
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mulaw_round_trips_within_its_step() {
        for s in (-32768..=32767).step_by(7) {
            let s = s as i16;
            let back = mulaw_decode(mulaw(s)) as i32;
            // half a step: the step is 1/16 of the segment, (|s| + bias) / 16
            let tol = ((s as i32).abs().min(32635) + 0x84) / 32 + 1;
            assert!((back - s as i32).abs() <= tol, "{s} -> {back}");
        }
        assert_eq!(mulaw_decode(mulaw(0)), 0);
    }

    #[test]
    fn bandwidth_follows_the_passband() {
        let usb = AudioSpec::new(Mode::Usb);
        assert_eq!(
            opus_band(audio_top(&usb), 12_000).0,
            opus1::Bandwidth::Narrowband
        );
        let cw = AudioSpec::new(Mode::Cw);
        assert_eq!(
            opus_band(audio_top(&cw), 12_000).0,
            opus1::Bandwidth::Narrowband
        );
        let am = AudioSpec::new(Mode::Am);
        assert_eq!(
            opus_band(audio_top(&am), 12_000).0,
            opus1::Bandwidth::Wideband
        );
        let wide = AudioSpec {
            lo_hz: -10_000.0,
            hi_hz: 10_000.0,
            ..am
        };
        assert_eq!(
            opus_band(audio_top(&wide), 24_000).0,
            opus1::Bandwidth::Superwideband
        );
    }

    /// Frames a decoder reads back: a 1 kHz tone through Opus at 12 and
    /// 24 kS/s, mono and stereo; mu-law and PCM16 as before.
    #[test]
    fn opus_frames_decode() {
        for (rate, chans) in [(12_000u32, 1usize), (24_000, 1), (12_000, 2)] {
            let n = (rate / 50) as usize * chans;
            let spec = AudioSpec::new(Mode::Usb);
            let mut e = WebEnc::default();
            e.set_codec(Codec::Opus);
            let ch = if chans == 2 {
                opus1::Channels::Stereo
            } else {
                opus1::Channels::Mono
            };
            let sr = opus1::SampleRate::try_from(rate as i32).unwrap();
            let mut d = opus1::OpusDecoder::new(sr, ch).unwrap();
            let mut out = Vec::new();
            let mut buf = vec![0f32; n];
            for k in 0..50 {
                let pcm: Vec<f32> = (0..n)
                    .map(|i| {
                        let t = (k * n + i) / chans;
                        0.3 * (2.0 * std::f32::consts::PI * 1000.0 * t as f32 / rate as f32).sin()
                    })
                    .collect();
                let f = e.frame(0, k as u32, rate, -50.0, &pcm, chans, &spec);
                assert_eq!(f[1] & FLAG_OPUS, FLAG_OPUS);
                assert_eq!(u16::from_le_bytes([f[2], f[3]]) as usize, n);
                // 24 kbit/s at 12 kS/s mono: 60 bytes a frame
                assert!(f.len() - AUDIO_HEADER < 200, "{}", f.len());
                let got = d.decode(&f[AUDIO_HEADER..], n / chans, &mut buf).unwrap();
                assert_eq!(got * chans, n);
                out.extend_from_slice(&buf[..n]);
            }
            // the tone comes back (after the decoder's start)
            let tail = &out[out.len() / 2..];
            let rms = (tail.iter().map(|v| v * v).sum::<f32>() / tail.len() as f32).sqrt();
            assert!(
                (rms / (0.3 / 2f32.sqrt()) - 1.0).abs() < 0.2,
                "{rate} {chans}: {rms}"
            );
        }
        let mut e = WebEnc::default();
        e.set_codec(Codec::Mulaw);
        let f = e.frame(
            0,
            0,
            12_000,
            -50.0,
            &[0.5, -0.5],
            1,
            &AudioSpec::new(Mode::Usb),
        );
        assert_eq!(f.len(), AUDIO_HEADER + 2);
        assert_eq!(f[1], FLAG_MULAW);
        assert!((mulaw_decode(f[AUDIO_HEADER]) as f32 / 32767.0 - 0.5).abs() < 0.02);
        e.set_codec(Codec::Pcm16);
        let f = e.frame(
            0,
            0,
            12_000,
            -50.0,
            &[0.5, -0.5],
            1,
            &AudioSpec::new(Mode::Usb),
        );
        assert_eq!(f.len(), AUDIO_HEADER + 4);
        assert_eq!(f[1], 0);
    }
}

/// `tests/proto/opus-tone.json` for the UI's decoder test (a 1 kHz tone,
/// USB, 12 kS/s mono): `UPDATE_FIXTURES=1` writes it. Opus output may
/// differ between machines, so it isn't compared, only decoded there.
#[cfg(test)]
#[test]
fn opus_fixture() {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/proto/opus-tone.json");
    if std::env::var_os("UPDATE_FIXTURES").is_none() {
        assert!(
            p.exists(),
            "{}: UPDATE_FIXTURES=1 cargo test -p web888-web opus_fixture",
            p.display()
        );
        return;
    }
    let mut e = WebEnc::default();
    e.set_codec(Codec::Opus);
    let spec = AudioSpec::new(Mode::Usb);
    let hex = |b: &[u8]| b.iter().map(|v| format!("{v:02x}")).collect::<String>();
    let frames: Vec<String> = (0..25)
        .map(|k| {
            let pcm: Vec<f32> = (0..240)
                .map(|i| {
                    0.3 * (2.0 * std::f32::consts::PI * 1000.0 * (k * 240 + i) as f32 / 12_000.0)
                        .sin()
                })
                .collect();
            format!(
                "\"{}\"",
                hex(&e.frame(0, k as u32, 12_000, -50.0, &pcm, 1, &spec))
            )
        })
        .collect();
    std::fs::write(
        &p,
        format!(
            "{{\"about\": \"web888d audio frames: Opus, a 1 kHz tone at 0.3, 12 kS/s mono (crates/web/src/codec.rs)\",\n \"frames\": [\n  {}\n ]}}\n",
            frames.join(",\n  ")
        ),
    )
    .unwrap();
}
