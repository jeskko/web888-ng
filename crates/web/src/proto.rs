// SPDX-License-Identifier: GPL-3.0-or-later
//! WebSocket protocol, version 2 (`doc/m2-web-ui.md` §4, with the M3
//! additions in `doc/m3-multi-user.md` §8: lease, users, share, listen;
//! M4's spots in `doc/m4-skimmers.md` §5):
//! JSON control in text frames, audio and waterfall data in binary frames
//! (little endian).
//! The TypeScript mirror is `ui/src/proto.ts`; `tests/proto/*.json` holds
//! fixtures both sides check against.

use serde::{Deserialize, Serialize};

pub const PROTO: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Usb,
    Lsb,
    Cw,
    Am,
}

impl From<Mode> for web888_dsp::audio::Mode {
    fn from(m: Mode) -> Self {
        use web888_dsp::audio::Mode as M;
        match m {
            Mode::Usb => M::Usb,
            Mode::Lsb => M::Lsb,
            Mode::Cw => M::Cw,
            Mode::Am => M::Am,
        }
    }
}

/// Client to server.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ClientMsg {
    Hello {
        proto: u32,
        #[serde(default)]
        codecs: Vec<String>,
    },
    /// Dial frequency (the carrier).
    Tune {
        hz: f64,
    },
    /// Mode and passband relative to the dial; missing values take the
    /// mode's defaults.
    Mode {
        mode: Mode,
        lo: Option<f64>,
        hi: Option<f64>,
        pitch: Option<f64>,
    },
    /// AGC on/off; `gain_db` is the fixed gain without AGC, the maximum
    /// gain with it. Noise blanker threshold and noise reduction strength
    /// (0: off), squelch on/off and threshold (channel dBFS).
    Audio {
        agc: Option<bool>,
        gain_db: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agc_speed: Option<AgcSpeed>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nb: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nr: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        squelch: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        squelch_db: Option<f32>,
    },
    Wf(WfReq),
    Ping {
        t: f64,
    },
    /// Show my frequency to others and let them listen along (default
    /// on), or not.
    Share {
        on: bool,
    },
    /// Listen along to another user's receiver (while waiting for one's
    /// own), or stop (`user: null`).
    Listen {
        user: Option<u64>,
    },
    /// Send the skimmers' spots: the recent ones, then new ones as they
    /// come (or stop).
    Spots {
        on: bool,
    },
}

/// SSB/CW AGC recovery speed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgcSpeed {
    Fast,
    #[default]
    Medium,
    Slow,
}

impl From<AgcSpeed> for web888_dsp::agc::AgcSpeed {
    fn from(s: AgcSpeed) -> Self {
        use web888_dsp::agc::AgcSpeed as S;
        match s {
            AgcSpeed::Fast => S::Fast,
            AgcSpeed::Medium => S::Medium,
            AgcSpeed::Slow => S::Slow,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WfReq {
    pub center_hz: f64,
    pub decim: u16,
    pub bins: usize,
    pub fps: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RadioInfo {
    pub backend: String,
    pub variant: String,
    pub adc_hz: f64,
    pub rx_rate: f64,
    pub rf_lo: f64,
    pub rf_hi: f64,
    pub wf_decim_min: u16,
    pub wf_decim_max: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    pub bins_max: usize,
    pub fps_max: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub hz: f64,
    pub mode: Mode,
    pub lo: f64,
    pub hi: f64,
    pub pitch: f64,
    pub agc: bool,
    pub gain_db: f32,
    pub agc_speed: AgcSpeed,
    /// Noise blanker threshold (impulse / mean level); 0: off.
    pub nb: f32,
    /// Noise reduction strength 0..1; 0: off.
    pub nr: f32,
    pub squelch: bool,
    /// Squelch threshold, channel power (dBFS).
    pub squelch_db: f32,
    pub wf: WfReq,
    /// Others see this user's frequency and may listen along.
    pub share: bool,
    /// The user listened along to (the dial follows theirs).
    pub listen: Option<u64>,
}

/// Another user's receiver, for markers on the scale.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct UserMark {
    pub id: u64,
    pub hz: f64,
    pub mode: Mode,
}

/// A decoded transmission from a skimmer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpotMsg {
    /// Slot start, UTC seconds.
    pub t: f64,
    pub mode: String,
    /// "40m", or "" outside the amateur bands.
    pub band: String,
    /// The skimmer's dial (USB).
    pub dial: f64,
    /// RF frequency of the signal (its lowest tone).
    pub hz: f64,
    pub snr: f32,
    pub dt: f32,
    pub msg: String,
    /// The sender and its locator, when the message says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid: Option<String>,
}

impl From<&web888_skim::Spot> for SpotMsg {
    fn from(s: &web888_skim::Spot) -> Self {
        SpotMsg {
            t: s.time,
            mode: s.mode.into(),
            band: web888_skim::spot::band_name(s.dial_hz).into(),
            dial: s.dial_hz,
            hz: s.freq_hz.round(),
            snr: s.snr_db.round(),
            dt: (s.dt_s * 10.0).round() / 10.0,
            msg: s.message.clone(),
            call: s.sender.clone(),
            grid: s.grid.clone(),
        }
    }
}

/// Server to client.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ServerMsg {
    Hello {
        proto: u32,
        server: String,
        codec: String,
        audio_rate: f64,
        radio: RadioInfo,
        limits: Limits,
        /// This session's id (as others see it in `users`).
        session: u64,
        class: String,
        /// The server runs skimmers (spots can be asked for).
        #[serde(default)]
        spots: bool,
    },
    State(State),
    /// The session's receiver: granted, or waiting at `position` (1 =
    /// next) in the queue.
    Lease {
        granted: bool,
        position: usize,
    },
    /// Other users who share their frequency (changes only).
    Users {
        users: Vec<UserMark>,
    },
    /// Spots, oldest first: the recent ones after a `spots` request, then
    /// each new batch.
    Spots {
        spots: Vec<SpotMsg>,
    },
    Status {
        adc_overloads: u64,
        clock_ppm: f64,
        audio_drops: u64,
        wf_drops: u64,
        hw_rate_ok: bool,
    },
    Pong {
        t: f64,
    },
    Busy {
        msg: String,
    },
    Error {
        msg: String,
    },
}

pub const KIND_AUDIO: u8 = 1;
pub const KIND_WF: u8 = 2;
pub const FLAG_OVERLOAD: u8 = 1;
/// Audio: samples were lost before this frame.
pub const FLAG_GAP: u8 = 2;
/// Waterfall: first line of a new tuning/zoom.
pub const FLAG_NEW_GEN: u8 = 2;
/// Waterfall: no new samples since the previous line (it repeats the
/// newest ones: a time-shared view, or a narrow span).
pub const FLAG_STALE: u8 = 4;

pub const AUDIO_HEADER: usize = 16;
pub const WF_HEADER: usize = 40;

/// Audio frame: header + PCM16 mono.
pub fn audio_frame(flags: u8, seq: u32, rate: u32, rssi_db: f32, pcm: &[f32]) -> Vec<u8> {
    let mut b = Vec::with_capacity(AUDIO_HEADER + 2 * pcm.len());
    b.push(KIND_AUDIO);
    b.push(flags);
    b.extend_from_slice(&(pcm.len() as u16).to_le_bytes());
    b.extend_from_slice(&seq.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&rssi_db.to_le_bytes());
    for &s in pcm {
        let v = (s * 32767.0).round().clamp(-32768.0, 32767.0) as i16;
        b.extend_from_slice(&v.to_le_bytes());
    }
    b
}

/// Waterfall line frame: header + u8 levels (`db0 + v * db_step`).
#[allow(clippy::too_many_arguments)]
pub fn wf_frame(
    flags: u8,
    seq: u32,
    first_hz: f64,
    bin_hz: f64,
    db0: f32,
    db_step: f32,
    time_ms: f64,
    levels: &[u8],
) -> Vec<u8> {
    let mut b = Vec::with_capacity(WF_HEADER + levels.len());
    b.push(KIND_WF);
    b.push(flags);
    b.extend_from_slice(&(levels.len() as u16).to_le_bytes());
    b.extend_from_slice(&seq.to_le_bytes());
    b.extend_from_slice(&first_hz.to_le_bytes());
    b.extend_from_slice(&bin_hz.to_le_bytes());
    b.extend_from_slice(&db0.to_le_bytes());
    b.extend_from_slice(&db_step.to_le_bytes());
    b.extend_from_slice(&time_ms.to_le_bytes());
    b.extend_from_slice(levels);
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_messages_parse() {
        let m: ClientMsg = serde_json::from_str(r#"{"type":"tune","hz":7074000}"#).unwrap();
        assert_eq!(m, ClientMsg::Tune { hz: 7_074_000.0 });
        let m: ClientMsg = serde_json::from_str(r#"{"type":"mode","mode":"cw"}"#).unwrap();
        assert_eq!(
            m,
            ClientMsg::Mode {
                mode: Mode::Cw,
                lo: None,
                hi: None,
                pitch: None
            }
        );
        let m: ClientMsg = serde_json::from_str(
            r#"{"type":"wf","center_hz":8.6e6,"decim":16,"bins":1024,"fps":20}"#,
        )
        .unwrap();
        assert!(matches!(m, ClientMsg::Wf(WfReq { decim: 16, .. })));
        assert!(serde_json::from_str::<ClientMsg>(r#"{"type":"nope"}"#).is_err());
    }

    #[test]
    fn frames_have_documented_layout() {
        let a = audio_frame(FLAG_GAP, 7, 12_000, -42.5, &[0.5, -1.0]);
        assert_eq!(a.len(), AUDIO_HEADER + 4);
        assert_eq!(&a[..4], &[1, 2, 2, 0]);
        assert_eq!(u32::from_le_bytes(a[4..8].try_into().unwrap()), 7);
        assert_eq!(u32::from_le_bytes(a[8..12].try_into().unwrap()), 12_000);
        assert_eq!(f32::from_le_bytes(a[12..16].try_into().unwrap()), -42.5);
        assert_eq!(i16::from_le_bytes([a[16], a[17]]), 16384);
        assert_eq!(i16::from_le_bytes([a[18], a[19]]), -32767);

        let w = wf_frame(FLAG_NEW_GEN, 3, 1e6, 7500.0, -160.0, 0.5, 1.5e12, &[0, 255]);
        assert_eq!(w.len(), WF_HEADER + 2);
        assert_eq!(&w[..4], &[2, 2, 2, 0]);
        assert_eq!(f64::from_le_bytes(w[8..16].try_into().unwrap()), 1e6);
        assert_eq!(f64::from_le_bytes(w[16..24].try_into().unwrap()), 7500.0);
        assert_eq!(f32::from_le_bytes(w[24..28].try_into().unwrap()), -160.0);
        assert_eq!(f64::from_le_bytes(w[32..40].try_into().unwrap()), 1.5e12);
        assert_eq!(&w[40..], &[0, 255]);
    }
}
