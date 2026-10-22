// SPDX-License-Identifier: GPL-3.0-or-later
//! One receiver's audio chain, from hardware RX samples to audio:
//!
//! ```text
//! Iq32 @ rx rate ─▶ decimate to 12 kS/s ─▶ channel filter (CIC droop folded in)
//!   ─▶ CW: shift by +pitch ─▶ demod ─▶ AGC or fixed gain ─▶ f32 audio
//! ```
//!
//! The hardware NCO sits on the dial frequency (the carrier); the passband
//! `lo..hi` is relative to it (USB 300..2700, LSB -2700..-300, CW -250..250
//! around the carrier, AM -5000..5000).

use crate::agc::{Agc, AgcSpec};
use crate::chan::{ChannelSpec, Droop, design};
use crate::decim::Decimator;
use crate::demod::{Am, ssb};
use crate::nco::Shifter;
use crate::ols::OverlapSave;
use crate::{C32, rx_to_c32};
use web888_radio::Iq32;

/// Audio sample rate.
pub const AUDIO_RATE: f64 = 12_000.0;
/// Widest passband edge (Hz from the carrier) the 12 kS/s chain supports.
pub const MAX_EDGE_HZ: f64 = 5_000.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Usb,
    Lsb,
    Cw,
    Am,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioSpec {
    pub mode: Mode,
    pub lo_hz: f64,
    pub hi_hz: f64,
    /// CW: audio frequency of a signal on the dial frequency.
    pub pitch_hz: f64,
    /// `None`: fixed gain `gain_db`.
    pub agc: Option<AgcSpec>,
    pub gain_db: f32,
}

impl AudioSpec {
    /// Default passband and AGC for `mode`.
    pub fn new(mode: Mode) -> Self {
        let (lo, hi, agc) = match mode {
            Mode::Usb => (300.0, 2700.0, AgcSpec::ssb()),
            Mode::Lsb => (-2700.0, -300.0, AgcSpec::ssb()),
            Mode::Cw => (-250.0, 250.0, AgcSpec::ssb()),
            Mode::Am => (-5000.0, 5000.0, AgcSpec::am()),
        };
        AudioSpec {
            mode,
            lo_hz: lo,
            hi_hz: hi,
            pitch_hz: 700.0,
            agc: Some(agc),
            gain_db: 0.0,
        }
    }

    /// Keep the passband within what the chain supports (at least 50 Hz
    /// wide, edges within +-`MAX_EDGE_HZ`) and the pitch audible.
    pub fn clamped(mut self) -> Self {
        self.lo_hz = self.lo_hz.clamp(-MAX_EDGE_HZ, MAX_EDGE_HZ - 50.0);
        self.hi_hz = self.hi_hz.clamp(self.lo_hz + 50.0, MAX_EDGE_HZ);
        self.pitch_hz = self.pitch_hz.clamp(200.0, 2000.0);
        self.gain_db = self.gain_db.clamp(-20.0, 100.0);
        self
    }

    fn channel(&self) -> ChannelSpec {
        ChannelSpec {
            lo_hz: self.lo_hz,
            hi_hz: self.hi_hz,
            transition_hz: if self.mode == Mode::Am { 500.0 } else { 200.0 },
            atten_db: 80.0,
        }
    }
}

pub struct AudioChain {
    spec: AudioSpec,
    droop: Droop,
    decim: Decimator,
    filter: OverlapSave,
    shift: Shifter,
    am: Am,
    agc: Option<Agc>,
    gain: f32,
    // scratch
    x: Vec<C32>,
    y: Vec<C32>,
    z: Vec<C32>,
    // channel power since the last `take_power`
    pow_sum: f64,
    pow_n: usize,
}

impl AudioChain {
    /// `rx_rate` must be an integer multiple of `AUDIO_RATE`; `droop` is the
    /// hardware channel response to compensate.
    pub fn new(spec: AudioSpec, rx_rate: f64, droop: Droop) -> Self {
        let r = (rx_rate / AUDIO_RATE).round() as usize;
        assert!(
            r >= 1 && (r as f64 * AUDIO_RATE - rx_rate).abs() < 1e-6,
            "RX rate {rx_rate} is not a multiple of {AUDIO_RATE}"
        );
        let spec = spec.clamped();
        AudioChain {
            spec,
            droop,
            decim: Decimator::new(rx_rate, r, MAX_EDGE_HZ + 300.0, 80.0),
            filter: Self::filter(&spec, droop),
            shift: Shifter::new(spec.pitch_hz, AUDIO_RATE),
            am: Am::new(AUDIO_RATE),
            agc: spec.agc.map(|a| Agc::new(a, AUDIO_RATE)),
            gain: Self::gain(&spec),
            x: Vec::new(),
            y: Vec::new(),
            z: Vec::new(),
            pow_sum: 0.0,
            pow_n: 0,
        }
    }

    fn filter(spec: &AudioSpec, droop: Droop) -> OverlapSave {
        let taps = design(spec.channel(), AUDIO_RATE, droop);
        // about 60 ms blocks instead of 150 ms with the default size
        let n = (2 * taps.len()).next_power_of_two().max(512);
        OverlapSave::with_fft_size(&taps, n)
    }

    fn gain(spec: &AudioSpec) -> f32 {
        10f32.powf(spec.gain_db / 20.0)
    }

    pub fn spec(&self) -> &AudioSpec {
        &self.spec
    }

    /// Change mode, passband or gain. The filter is redesigned only if the
    /// passband changed; the AGC restarts on a mode or AGC change.
    pub fn set_spec(&mut self, spec: AudioSpec) {
        let spec = spec.clamped();
        let old = self.spec;
        if spec.channel() != old.channel() {
            self.filter = Self::filter(&spec, self.droop);
        }
        if spec.pitch_hz != old.pitch_hz {
            self.shift = Shifter::new(spec.pitch_hz, AUDIO_RATE);
        }
        if spec.mode != old.mode || spec.agc != old.agc {
            self.agc = spec.agc.map(|a| Agc::new(a, AUDIO_RATE));
            self.am = Am::new(AUDIO_RATE);
        }
        self.gain = Self::gain(&spec);
        self.spec = spec;
    }

    /// Mean channel power (dBFS, before AGC) since the last call, if any
    /// samples came through.
    pub fn take_power_db(&mut self) -> Option<f32> {
        if self.pow_n == 0 {
            return None;
        }
        let p = self.pow_sum / self.pow_n as f64;
        self.pow_sum = 0.0;
        self.pow_n = 0;
        Some((10.0 * p.max(1e-20).log10()) as f32)
    }

    /// AGC gain in dB (the fixed gain without AGC).
    pub fn gain_db(&self) -> f32 {
        match &self.agc {
            Some(a) => a.gain_db(),
            None => self.spec.gain_db,
        }
    }

    /// Process RX samples of this channel, appending audio to `out`.
    pub fn process(&mut self, rx: impl IntoIterator<Item = Iq32>, out: &mut Vec<f32>) {
        self.x.clear();
        self.x.extend(rx.into_iter().map(rx_to_c32));
        self.y.clear();
        self.decim.process(&self.x, &mut self.y);
        self.z.clear();
        self.filter.process(&self.y, &mut self.z);
        for s in &self.z {
            self.pow_sum += s.norm_sqr() as f64;
        }
        self.pow_n += self.z.len();
        let start = out.len();
        match self.spec.mode {
            Mode::Usb | Mode::Lsb => ssb(&self.z, out),
            Mode::Cw => {
                self.shift.process(&mut self.z);
                ssb(&self.z, out);
            }
            Mode::Am => self.am.process(&self.z, out),
        }
        let a = &mut out[start..];
        match &mut self.agc {
            Some(agc) => agc.process(a),
            None => a
                .iter_mut()
                .for_each(|x| *x = (*x * self.gain).clamp(-1.0, 1.0)),
        }
    }
}
