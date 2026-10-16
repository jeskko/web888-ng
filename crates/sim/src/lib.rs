// SPDX-License-Identifier: GPL-3.0-or-later
//! Simulated Web-888: a [`Radio`] backend that synthesises what the FPGA
//! would deliver for a given RF scene.
//!
//! Channel model (per RX channel / WF slot, see `web888_radio::chan`):
//! * each real tone at `f` appears at `f - f_nco` (and its image at
//!   `-f - f_nco`), where `f_nco` comes from the 48-bit NCO word. The word
//!   is computed from the rate the caller *assumes* (`set_adc_hz`), while the
//!   samples run on the *true* ADC rate (`SimConfig::adc_ppm`), so clock
//!   errors and PPS correction behave like on hardware;
//! * the exact CIC magnitude response of the chain, aliasing by sampling at
//!   the real output rate, stock full-scale scaling, i32/i16 quantisation;
//! * white ADC noise (`Scene::noise_dbfs_hz`).
//!
//! Timing: [`Pacing::RealTime`] produces samples at the true rate against
//! the wall clock, with a ring of the stock size that overruns (counted) if
//! the reader falls behind, and a PPS period once per second.
//! [`Pacing::Free`] makes every read return a full buffer (tests, offline
//! runs). Simulated time then advances with the RX samples read.

mod rng;
mod scene;

pub use scene::{Scene, Source};

use num_complex::Complex64;
use std::time::Instant;
use web888_radio::*;

use rng::Rng;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pacing {
    RealTime,
    Free,
}

#[derive(Clone, Debug)]
pub struct SimConfig {
    pub variant: Variant,
    /// Nominal ADC rate (what the hardware is "supposed" to run at).
    pub adc_hz: f64,
    /// True ADC rate error in ppm (the crystal is off by this much).
    pub adc_ppm: f64,
    pub rx_channels: usize,
    pub pacing: Pacing,
    pub scene: Scene,
    pub seed: u64,
}

impl SimConfig {
    /// Stock HF unit: 122.88 MHz, 12 RX channels.
    pub fn hf(scene: Scene, pacing: Pacing) -> Self {
        SimConfig {
            variant: Variant::Hf,
            adc_hz: ADC_HZ_HF,
            adc_ppm: 0.0,
            rx_channels: 12,
            pacing,
            scene,
            seed: 1,
        }
    }
}

/// Counters for things the real hardware would lose.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SimStats {
    /// RX frames overwritten because the reader was too slow.
    pub rx_overrun_frames: u64,
}

/// RX ring size of the stock driver (bytes).
const RING_BYTES: usize = 0x7FC00;
const WF_SLOTS: usize = 2;

/// One synthetic spectral line as seen by one channel.
#[derive(Clone, Copy, Debug)]
struct Line {
    phasor: Complex64,
    rot: Complex64,
}

#[derive(Clone, Debug, Default)]
struct Chan {
    /// NCO word as programmed.
    word: u64,
    lines: Vec<Line>,
}

#[derive(Clone, Debug)]
struct WfSlot {
    chan: Chan,
    decim: u16,
    mode: WfMode,
    started: Option<Instant>,
    produced: u64,
    active: bool,
}

pub struct SimRadio {
    cfg: SimConfig,
    /// Rate assumed by the caller for NCO words.
    adc_assumed: f64,
    r2: u16,
    rx: Vec<Chan>,
    wf: Vec<WfSlot>,
    fe: FrontEnd,
    rng: Rng,
    // RX timing
    rx_t0: Instant,
    rx_produced: u64,
    // PPS
    pps_emitted: u64,
    stats: SimStats,
}

impl SimRadio {
    pub fn new(cfg: SimConfig) -> Self {
        let n = cfg.rx_channels;
        let mut s = SimRadio {
            adc_assumed: cfg.adc_hz,
            r2: 20,
            rx: vec![Chan::default(); n],
            wf: (0..WF_SLOTS)
                .map(|_| WfSlot {
                    chan: Chan::default(),
                    decim: 1,
                    mode: WfMode::OneShot,
                    started: None,
                    produced: 0,
                    active: false,
                })
                .collect(),
            fe: FrontEnd::default(),
            rng: Rng::new(cfg.seed),
            rx_t0: Instant::now(),
            rx_produced: 0,
            pps_emitted: 0,
            stats: SimStats::default(),
            cfg,
        };
        for ch in 0..n {
            s.rebuild_rx(ch);
        }
        s
    }

    pub fn stats(&self) -> SimStats {
        self.stats
    }

    pub fn scene_mut(&mut self) -> &mut Scene {
        &mut self.cfg.scene
    }

    /// Re-apply the scene after `scene_mut` changes.
    pub fn refresh(&mut self) {
        for ch in 0..self.rx.len() {
            self.rebuild_rx(ch);
        }
        for s in 0..WF_SLOTS {
            self.rebuild_wf(s);
        }
    }

    fn adc_true(&self) -> f64 {
        self.cfg.adc_hz * (1.0 + self.cfg.adc_ppm * 1e-6)
    }

    fn rx_fs_out(&self) -> f64 {
        rx_rate(self.adc_true(), self.r2)
    }

    fn signal_gain(&self) -> f64 {
        10f64.powf(-(self.fe.atten_db as f64) / 20.0)
    }

    /// Spectral lines for a channel: each tone and its image, offset from
    /// the actual NCO frequency, scaled by the chain response.
    fn lines(
        &self,
        word: u64,
        fs_out: f64,
        mag: impl Fn(f64) -> f64,
        full_scale: f64,
    ) -> Vec<Line> {
        let fs = self.adc_true();
        let f_nco = nco_freq(word, fs);
        let g = self.signal_gain();
        let mut v = Vec::new();
        for src in &self.cfg.scene.sources {
            let Source::Tone { f_hz, dbfs, phase } = *src;
            // real tone -> two complex lines (+f and -f) after mixing
            for (fl, ph) in [(f_hz, phase), (-f_hz, -phase)] {
                let d = alias(fl - f_nco, fs); // offset at the ADC rate
                let m = mag(d);
                if m < 1e-9 {
                    continue;
                }
                let amp = full_scale * 10f64.powf(dbfs / 20.0) * g * m;
                let w = 2.0 * std::f64::consts::PI * d / fs_out;
                v.push(Line {
                    phasor: Complex64::from_polar(amp, ph),
                    rot: Complex64::from_polar(1.0, w),
                });
            }
        }
        v
    }

    fn rebuild_rx(&mut self, ch: usize) {
        let fs = self.adc_true();
        let r2 = self.r2;
        let fs_out = self.rx_fs_out();
        let word = self.rx[ch].word;
        let lines = self.lines(
            word,
            fs_out,
            |d| rx_chain_mag(d, fs, r2),
            RX_SINE_FULL_SCALE,
        );
        self.rx[ch].lines = lines;
    }

    fn rebuild_wf(&mut self, slot: usize) {
        let fs = self.adc_true();
        let decim = self.wf[slot].decim;
        let fs_out = fs / decim as f64;
        let word = self.wf[slot].chan.word;
        let lines = self.lines(
            word,
            fs_out,
            |d| wf_chain_mag(d, fs, decim),
            WF_SINE_FULL_SCALE,
        );
        self.wf[slot].chan.lines = lines;
    }

    /// Per-component noise sigma for output rate `fs_out` at `full_scale`.
    fn noise_sigma(&self, fs_out: f64, full_scale: f64) -> f64 {
        match self.cfg.scene.noise_dbfs_hz {
            None => 0.0,
            Some(n0) => (10f64.powf(n0 / 10.0) * fs_out / 2.0).sqrt() * full_scale,
        }
    }

    fn rx_ring_frames(&self) -> u64 {
        (RING_BYTES / (8 * self.rx.len())) as u64
    }

    /// RX frames the hardware has produced and not yet handed out.
    fn rx_available(&mut self, want: usize) -> usize {
        match self.cfg.pacing {
            Pacing::Free => want,
            Pacing::RealTime => {
                let due = (self.rx_t0.elapsed().as_secs_f64() * self.rx_fs_out()) as u64;
                let mut avail = due.saturating_sub(self.rx_produced);
                let cap = self.rx_ring_frames();
                if avail > cap {
                    // the FPGA overwrote what we did not read in time
                    let lost = avail - cap;
                    self.stats.rx_overrun_frames += lost;
                    self.skip_rx(lost);
                    avail = cap;
                }
                avail.min(want as u64) as usize
            }
        }
    }

    /// Advance all RX channels by `n` frames without producing output.
    fn skip_rx(&mut self, n: u64) {
        for c in &mut self.rx {
            for l in &mut c.lines {
                l.phasor *= l.rot.powf(n as f64);
            }
        }
        self.rx_produced += n;
    }

    /// Simulated time in seconds.
    fn sim_time(&self) -> f64 {
        match self.cfg.pacing {
            Pacing::RealTime => self.rx_t0.elapsed().as_secs_f64(),
            Pacing::Free => self.rx_produced as f64 / self.rx_fs_out(),
        }
    }
}

fn q32(x: f64) -> i32 {
    x.round().clamp(i32::MIN as f64, i32::MAX as f64) as i32
}

fn q16(x: f64) -> i16 {
    x.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16
}

impl Radio for SimRadio {
    fn caps(&self) -> Caps {
        Caps {
            variant: self.cfg.variant,
            rx_channels: self.rx.len(),
            wf_channels: WF_SLOTS,
            adc_hz: self.cfg.adc_hz,
            rx_r2: 5..=40,
            wf_decim_max: 8192,
            wf_block: WF_BLOCK,
            sample_counter: true,
        }
    }

    fn adc_hz(&self) -> f64 {
        self.adc_assumed
    }

    fn set_adc_hz(&mut self, hz: f64) {
        self.adc_assumed = hz;
    }

    fn set_rx_rate(&mut self, r2: u16) -> Result<()> {
        if !self.caps().rx_r2.contains(&r2) {
            return Err(Error::InvalidParam("r2 out of range"));
        }
        self.r2 = r2;
        self.rx_t0 = Instant::now();
        self.rx_produced = 0;
        self.pps_emitted = 0;
        for ch in 0..self.rx.len() {
            self.rebuild_rx(ch);
        }
        Ok(())
    }

    fn rx_r2(&self) -> u16 {
        self.r2
    }

    fn tune_rx(&mut self, ch: usize, f_hz: f64) -> Result<()> {
        if ch >= self.rx.len() {
            return Err(Error::InvalidChannel(ch));
        }
        self.rx[ch].word = nco_word(f_hz, self.adc_assumed);
        self.rebuild_rx(ch);
        Ok(())
    }

    fn read_rx(&mut self, buf: &mut [Iq32]) -> Result<usize> {
        let n = self.rx.len();
        if !buf.len().is_multiple_of(n) {
            return Err(Error::BadBuffer);
        }
        let frames = self.rx_available(buf.len() / n);
        let sigma = self.noise_sigma(self.rx_fs_out(), RX_SINE_FULL_SCALE);
        for f in 0..frames {
            for (c, chan) in self.rx.iter_mut().enumerate() {
                let mut z = Complex64::new(0.0, 0.0);
                for l in &mut chan.lines {
                    z += l.phasor;
                    l.phasor *= l.rot;
                }
                if sigma > 0.0 {
                    let (a, b) = self.rng.gauss2();
                    z += Complex64::new(a * sigma, b * sigma);
                }
                buf[f * n + c] = Iq32 {
                    i: q32(z.re),
                    q: q32(z.im),
                };
            }
        }
        // keep phasor amplitudes from drifting over long runs
        if self.rx_produced / 4096 != (self.rx_produced + frames as u64) / 4096 {
            self.refresh_rx_amplitudes();
        }
        self.rx_produced += frames as u64;
        Ok(frames)
    }

    fn start_wf(&mut self, slot: usize, f_hz: f64, decim: u16, mode: WfMode) -> Result<()> {
        if slot >= WF_SLOTS {
            return Err(Error::InvalidChannel(slot));
        }
        if decim == 0 || decim > 8192 || !decim.is_power_of_two() {
            return Err(Error::InvalidParam(
                "WF decimation must be a power of two 1..=8192",
            ));
        }
        let word = nco_word(f_hz, self.adc_assumed);
        let s = &mut self.wf[slot];
        s.chan.word = word;
        s.decim = decim;
        s.mode = mode;
        s.started = Some(Instant::now());
        s.produced = 0;
        s.active = true;
        self.rebuild_wf(slot);
        Ok(())
    }

    fn read_wf(&mut self, slot: usize, buf: &mut [Iq16]) -> Result<usize> {
        if slot >= WF_SLOTS {
            return Err(Error::InvalidChannel(slot));
        }
        let fs_out = self.adc_true() / self.wf[slot].decim as f64;
        let sigma = self.noise_sigma(fs_out, WF_SINE_FULL_SCALE);
        let pacing = self.cfg.pacing;
        let s = &mut self.wf[slot];
        if !s.active {
            return Ok(0);
        }
        let mut avail = match (pacing, s.started) {
            (Pacing::RealTime, Some(t0)) => {
                ((t0.elapsed().as_secs_f64() * fs_out) as u64).saturating_sub(s.produced)
            }
            _ => buf.len() as u64,
        };
        if s.mode == WfMode::OneShot {
            avail = avail.min(WF_BLOCK as u64 - s.produced);
        }
        let n = (avail as usize).min(buf.len());
        for out in buf.iter_mut().take(n) {
            let mut z = Complex64::new(0.0, 0.0);
            for l in &mut s.chan.lines {
                z += l.phasor;
                l.phasor *= l.rot;
            }
            if sigma > 0.0 {
                let (a, b) = self.rng.gauss2();
                z += Complex64::new(a * sigma, b * sigma);
            }
            *out = Iq16 {
                i: q16(z.re),
                q: q16(z.im),
            };
        }
        s.produced += n as u64;
        if s.mode == WfMode::OneShot && s.produced >= WF_BLOCK as u64 {
            s.active = false;
        }
        Ok(n)
    }

    fn front_end(&mut self, fe: FrontEnd) -> Result<()> {
        if !(0.0..=31.5).contains(&fe.atten_db) {
            return Err(Error::InvalidParam("attenuation 0..31.5 dB"));
        }
        if fe.antenna > 0x3f {
            return Err(Error::InvalidParam("antenna is 6 bits"));
        }
        self.fe = fe;
        self.refresh();
        Ok(())
    }

    fn pps(&mut self) -> Result<Option<u32>> {
        let secs = self.sim_time() as u64;
        if secs > self.pps_emitted {
            self.pps_emitted += 1;
            Ok(Some(self.adc_true().round() as u32))
        } else {
            Ok(None)
        }
    }

    fn sample_counter(&mut self) -> Option<u64> {
        Some((self.sim_time() * self.adc_true()) as u64)
    }

    fn identity(&self) -> Identity {
        Identity {
            backend: "sim",
            signature: 0x5100_0200 | self.rx.len() as u32,
            dna: Some(0x1_2345_6789_abcd),
            build_id: None,
        }
    }
}

impl SimRadio {
    fn refresh_rx_amplitudes(&mut self) {
        // recompute exact amplitudes, keep phases
        for ch in 0..self.rx.len() {
            let phases: Vec<f64> = self.rx[ch].lines.iter().map(|l| l.phasor.arg()).collect();
            self.rebuild_rx(ch);
            for (l, ph) in self.rx[ch].lines.iter_mut().zip(phases) {
                l.phasor = Complex64::from_polar(l.phasor.norm(), ph);
            }
        }
    }
}
