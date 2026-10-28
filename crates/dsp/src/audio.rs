// SPDX-License-Identifier: GPL-3.0-or-later
//! One receiver's audio chain, from hardware RX samples to audio:
//!
//! ```text
//! Iq32 @ rx rate ─▶ NB ─▶ decimate to 12 kS/s ─▶ channel filter (CIC droop
//!   folded in; its power is the S-meter and the squelch's input)
//!   ─▶ CW: shift by +pitch ─▶ demod (AM with AGC: carrier-referenced)
//!   ─▶ NR ─▶ SSB/CW: AGC, or fixed gain ─▶ squelch ─▶ f32 audio
//! ```
//!
//! The hardware NCO sits on the dial frequency (the carrier); the passband
//! `lo..hi` is relative to it (USB 300..2700, LSB -2700..-300, CW -250..250
//! around the carrier, AM -5000..5000).
//!
//! Synchronous AM (`demod::Sync`): SAM both sidebands; SAL and SAR one,
//! the channel filter (-5000..50, -50..5000) taking out the other; SAS
//! stereo (lower sideband left, upper right), interleaved, two channels.

use crate::agc::{Agc, AgcSpec};
use crate::chan::{ChannelSpec, Droop, design};
use crate::decim::Decimator;
use crate::demod::{Am, Sync, SyncOut, ssb};
use crate::nb::NoiseBlanker;
use crate::nco::Shifter;
use crate::nr::NoiseReduction;
use crate::ols::OverlapSave;
use crate::squelch::Squelch;
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
    /// Synchronous AM: both sidebands, lower, upper, stereo.
    Sam,
    Sal,
    Sar,
    Sas,
}

impl Mode {
    /// AM, envelope or synchronous (a carrier, carrier-referenced AGC).
    pub fn is_am(self) -> bool {
        matches!(
            self,
            Mode::Am | Mode::Sam | Mode::Sal | Mode::Sar | Mode::Sas
        )
    }

    /// Audio channels (interleaved).
    pub fn channels(self) -> usize {
        if self == Mode::Sas { 2 } else { 1 }
    }
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
    /// Noise blanker threshold (impulse / mean magnitude); `None`: off.
    pub nb: Option<f32>,
    /// Noise reduction strength 0..1 (0: off).
    pub nr: f32,
    /// Squelch threshold (channel power, dBFS); `None`: off.
    pub squelch_db: Option<f32>,
}

impl AudioSpec {
    /// Default passband and AGC for `mode`.
    pub fn new(mode: Mode) -> Self {
        let (lo, hi, agc) = match mode {
            Mode::Usb => (300.0, 2700.0, AgcSpec::ssb()),
            Mode::Lsb => (-2700.0, -300.0, AgcSpec::ssb()),
            Mode::Cw => (-250.0, 250.0, AgcSpec::ssb()),
            Mode::Am | Mode::Sam | Mode::Sas => (-5000.0, 5000.0, AgcSpec::am()),
            // the carrier stays inside, 50 Hz from the edge
            Mode::Sal => (-5000.0, 50.0, AgcSpec::am()),
            Mode::Sar => (-50.0, 5000.0, AgcSpec::am()),
        };
        AudioSpec {
            mode,
            lo_hz: lo,
            hi_hz: hi,
            pitch_hz: 700.0,
            agc: Some(agc),
            gain_db: 0.0,
            nb: None,
            nr: 0.0,
            squelch_db: None,
        }
    }

    /// Keep the passband within what the chain supports (at least 50 Hz
    /// wide, edges within +-`MAX_EDGE_HZ`) and the pitch audible.
    pub fn clamped(mut self) -> Self {
        self.lo_hz = self.lo_hz.clamp(-MAX_EDGE_HZ, MAX_EDGE_HZ - 50.0);
        self.hi_hz = self.hi_hz.clamp(self.lo_hz + 50.0, MAX_EDGE_HZ);
        self.pitch_hz = self.pitch_hz.clamp(200.0, 2000.0);
        self.gain_db = self.gain_db.clamp(-20.0, 100.0);
        self.nb = self.nb.map(|k| k.clamp(2.0, 50.0));
        self.nr = if self.nr.is_finite() {
            self.nr.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.squelch_db = self.squelch_db.map(|d| d.clamp(-160.0, 0.0));
        self
    }

    fn channel(&self) -> ChannelSpec {
        ChannelSpec {
            lo_hz: self.lo_hz,
            hi_hz: self.hi_hz,
            // AM's wider transitions are shorter filters; one sideband
            // needs a sharp edge by the carrier
            transition_hz: if matches!(self.mode, Mode::Am | Mode::Sam | Mode::Sas) {
                500.0
            } else {
                200.0
            },
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
    sync: Sync,
    agc: Option<Agc>,
    gain: f32,
    nb: Option<NoiseBlanker>,
    nr: Option<NoiseReduction>,
    /// SAS: the right channel's.
    nr_r: Option<NoiseReduction>,
    squelch: Option<Squelch>,
    // scratch
    x: Vec<C32>,
    y: Vec<C32>,
    z: Vec<C32>,
    pw: Vec<f32>,
    l: Vec<f32>,
    r: Vec<f32>,
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
            sync: Sync::new(AUDIO_RATE),
            agc: spec.agc.map(|a| Agc::new(a, AUDIO_RATE)),
            gain: Self::gain(&spec),
            nb: spec.nb.map(NoiseBlanker::new),
            nr: (spec.nr > 0.0).then(|| NoiseReduction::new(spec.nr)),
            nr_r: (spec.nr > 0.0).then(|| NoiseReduction::new(spec.nr)),
            squelch: spec.squelch_db.map(|d| Squelch::new(d, AUDIO_RATE)),
            x: Vec::new(),
            y: Vec::new(),
            z: Vec::new(),
            pw: Vec::new(),
            l: Vec::new(),
            r: Vec::new(),
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
            if !(spec.mode.is_am() && old.mode.is_am()) {
                // between AM modes the loop stays locked
                self.sync = Sync::new(AUDIO_RATE);
            }
        }
        self.gain = Self::gain(&spec);
        match (spec.nb, &mut self.nb) {
            (None, _) => self.nb = None,
            (Some(k), Some(nb)) => nb.set_threshold(k),
            (Some(k), None) => self.nb = Some(NoiseBlanker::new(k)),
        }
        for nr in [&mut self.nr, &mut self.nr_r] {
            match (spec.nr > 0.0, nr.as_mut()) {
                (false, _) => *nr = None,
                (true, Some(n)) => n.set_strength(spec.nr),
                (true, None) => *nr = Some(NoiseReduction::new(spec.nr)),
            }
        }
        match (spec.squelch_db, &mut self.squelch) {
            (None, _) => self.squelch = None,
            (Some(d), Some(sq)) => sq.set_threshold(d),
            (Some(d), None) => self.squelch = Some(Squelch::new(d, AUDIO_RATE)),
        }
        self.spec = spec;
    }

    /// The squelch is on and closed.
    pub fn squelched(&self) -> bool {
        self.squelch.as_ref().is_some_and(|s| !s.is_open())
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

    /// Audio channels the chain outputs (interleaved).
    pub fn channels(&self) -> usize {
        self.spec.mode.channels()
    }

    /// SAM modes: the carrier's offset from the dial (Hz) the loop
    /// follows.
    pub fn carrier_offset_hz(&self) -> Option<f64> {
        matches!(
            self.spec.mode,
            Mode::Sam | Mode::Sal | Mode::Sar | Mode::Sas
        )
        .then(|| self.sync.offset_hz(AUDIO_RATE))
    }

    /// Process RX samples of this channel, appending audio to `out`
    /// (`channels()` interleaved).
    pub fn process(&mut self, rx: impl IntoIterator<Item = Iq32>, out: &mut Vec<f32>) {
        self.process_c32(rx.into_iter().map(rx_to_c32), out);
    }

    /// As `process`, with samples scaled as `rx_to_c32` does (a full-scale
    /// sine has |z| = 1).
    pub fn process_c32(&mut self, x: impl IntoIterator<Item = C32>, out: &mut Vec<f32>) {
        self.x.clear();
        self.x.extend(x);
        if let Some(nb) = &mut self.nb {
            nb.process(&mut self.x);
        }
        self.y.clear();
        self.decim.process(&self.x, &mut self.y);
        self.z.clear();
        self.filter.process(&self.y, &mut self.z);
        self.pw.clear();
        self.pw.extend(self.z.iter().map(|s| s.norm_sqr()));
        self.pow_sum += self.pw.iter().map(|&p| p as f64).sum::<f64>();
        self.pow_n += self.z.len();
        let start = out.len();
        let carrier_agc = self.spec.mode.is_am() && self.agc.is_some();
        let agc = self
            .spec
            .agc
            .map(|a| (a.target, 10f32.powf(-a.max_gain_db / 20.0)));
        match self.spec.mode {
            Mode::Usb | Mode::Lsb => ssb(&self.z, out),
            Mode::Cw => {
                self.shift.process(&mut self.z);
                ssb(&self.z, out);
            }
            Mode::Am => match self.spec.agc {
                Some(a) => {
                    let min_carrier = 10f32.powf(-a.max_gain_db / 20.0);
                    self.am
                        .process_carrier_agc(&self.z, out, a.target, min_carrier)
                }
                None => self.am.process(&self.z, out),
            },
            Mode::Sam | Mode::Sal | Mode::Sar | Mode::Sas => {
                let kind = match self.spec.mode {
                    Mode::Sam => SyncOut::Both,
                    Mode::Sas => SyncOut::Stereo,
                    _ => SyncOut::Single,
                };
                self.sync.process(&self.z, kind, agc, out);
            }
        }
        let a = &mut out[start..];
        if self.channels() == 2 {
            if let (Some(nl), Some(nr)) = (&mut self.nr, &mut self.nr_r) {
                self.l.clear();
                self.r.clear();
                for f in a.chunks(2) {
                    self.l.push(f[0]);
                    self.r.push(f[1]);
                }
                nl.process(&mut self.l);
                nr.process(&mut self.r);
                for ((f, l), r) in a.chunks_mut(2).zip(&self.l).zip(&self.r) {
                    f[0] = *l;
                    f[1] = *r;
                }
            }
        } else if let Some(nr) = &mut self.nr {
            nr.process(a);
        }
        match &mut self.agc {
            Some(_) if carrier_agc => {}
            Some(agc) => agc.process(a),
            None => a
                .iter_mut()
                .for_each(|x| *x = (*x * self.gain).clamp(-1.0, 1.0)),
        }
        if let Some(sq) = &mut self.squelch {
            sq.process_frames(&self.pw, a, self.spec.mode.channels());
        }
    }
}
