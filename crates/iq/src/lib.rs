// SPDX-License-Identifier: GPL-3.0-or-later
//! [`Radio`] backend fed by real signals: a wideband complex IQ stream
//! from an SDR (SoapySDR, e.g. a HackRF on an HF antenna) or a recording,
//! channelized like the Web-888 FPGA (see [`ddc`]).
//!
//! The input covers `rate` around `center`, instead of the Web-888's
//! 0..61 MHz. Channels can be tuned up to one `rate` beyond either side:
//! the offset wraps the way complex sampling does, so a channel there
//! hears what the SDR's anti-alias filter let fold into the band, i.e.
//! the real signal at that frequency, attenuated by the filter skirt (a
//! test aid; within +-0.45 x rate is the clean band). The input rate must
//! be the ADC clock (122.88 MHz) divided by a power of two from 8 to 256,
//! so that every RX and WF rate divides it: 7.68 MS/s (/16) is the natural
//! choice for a HackRF. WF decimations below that divisor are refused.
//!
//! Levels: a complex input sine with |z| = 1 counts as 0 dBFS, i.e. an RX
//! channel then reads [`RX_SINE_FULL_SCALE`] like the hardware for a
//! full-scale tone. Input samples with |I| or |Q| near full scale count as
//! ADC overflows ([`Radio::hw_stats`]).
//!
//! Timing follows the source (a device, or a file paced at its rate). RX
//! output waits in a ring of the stock size; a reader that falls behind
//! loses the oldest frames (counted as overruns, like the open driver).

pub mod bins;
pub mod ddc;
pub mod source;

use ddc::Ddc;
use num_complex::{Complex32, Complex64};
use source::{Feed, IqSource};
use std::collections::VecDeque;
use std::time::Instant;
use web888_radio::*;

/// RX ring of the stock driver (bytes).
const RING_BYTES: usize = 0x7FC00;
const WF_SLOTS: usize = 2;
/// |I| or |Q| at or above this counts as an ADC overflow.
const CLIP: f32 = 0.99;

#[derive(Clone, Debug)]
pub struct IqConfig {
    /// Centre frequency of the input (Hz).
    pub center_hz: f64,
    /// Input sample rate: ADC clock / 2^k, k = 3..=8.
    pub rate_hz: f64,
    pub rx_channels: usize,
    /// Remove the input's DC offset (SDR LO leakage).
    pub dc_block: bool,
}

impl IqConfig {
    /// 12 RX channels, DC blocker on.
    pub fn new(center_hz: f64, rate_hz: f64) -> Self {
        IqConfig {
            center_hz,
            rate_hz,
            rx_channels: 12,
            dc_block: true,
        }
    }

    /// ADC clock / input rate, if valid.
    pub fn divisor(&self) -> Option<u32> {
        let m = ADC_HZ_HF / self.rate_hz;
        let mr = m.round();
        ((m - mr).abs() < 1e-6 && (8.0..=256.0).contains(&mr) && (mr as u32).is_power_of_two())
            .then_some(mr as u32)
    }
}

/// Where the input comes from.
enum Input {
    /// A source thread (real time).
    Feed(Feed),
    /// Read on demand: every read gets as much output as it asks for
    /// (tests, offline processing).
    Direct(Box<dyn IqSource>),
}

struct RxChan {
    word: u64,
    /// Tuned and not idled since: only such channels are computed.
    tuned: bool,
    ddc: Option<Ddc>,
}

struct WfSlot {
    word: u64,
    decim: u16,
    mode: WfMode,
    ddc: Option<Ddc>,
    q: VecDeque<Iq16>,
    produced: u64,
    active: bool,
    gaps: u64,
    /// Continuous: the ring was full and input went by uncomputed.
    skipped: bool,
}

pub struct IqRadio {
    cfg: IqConfig,
    m: u32,
    input: Input,
    adc_assumed: f64,
    r2: u16,
    rx: Vec<RxChan>,
    /// Input samples since the RX (re)start, for decimation phase.
    rx_k: u64,
    /// scratch: input after DC removal, one chain's outputs
    pre: Vec<Complex32>,
    out: Vec<Complex64>,
    rx_q: VecDeque<Iq32>,
    wf: Vec<WfSlot>,
    gain: f64,
    dc: Complex32,
    consumed: u64,
    t0: Instant,
    pps_emitted: u64,
    stats: HwStats,
    /// The source is a device (real time), not a recording.
    live: bool,
}

impl IqRadio {
    /// Real-time radio on a source thread.
    pub fn new(cfg: IqConfig, src: Box<dyn IqSource>) -> Result<Self> {
        let live = src.realtime();
        let feed = Feed::spawn(src, cfg.rate_hz);
        let mut r = Self::with_input(cfg, Input::Feed(feed))?;
        r.live = live;
        Ok(r)
    }

    /// Radio that reads the source on demand (no pacing).
    pub fn direct(cfg: IqConfig, src: Box<dyn IqSource>) -> Result<Self> {
        Self::with_input(cfg, Input::Direct(src))
    }

    fn with_input(cfg: IqConfig, input: Input) -> Result<Self> {
        let m = cfg.divisor().ok_or(Error::InvalidParam(
            "IQ input rate must be 122.88 MHz / 8, 16, ..., 256 (e.g. 7.68e6)",
        ))?;
        let n = cfg.rx_channels;
        let mut r = IqRadio {
            m,
            input,
            adc_assumed: ADC_HZ_HF,
            r2: 20,
            rx: (0..n)
                .map(|_| RxChan {
                    word: 0,
                    tuned: false,
                    ddc: None,
                })
                .collect(),
            rx_k: 0,
            pre: Vec::new(),
            out: Vec::new(),
            rx_q: VecDeque::new(),
            wf: (0..WF_SLOTS)
                .map(|_| WfSlot {
                    word: 0,
                    decim: 1,
                    mode: WfMode::OneShot,
                    ddc: None,
                    q: VecDeque::new(),
                    produced: 0,
                    active: false,
                    gaps: 0,
                    skipped: false,
                })
                .collect(),
            gain: 1.0,
            dc: Complex32::default(),
            consumed: 0,
            t0: Instant::now(),
            pps_emitted: 0,
            stats: HwStats::default(),
            live: false,
            cfg,
        };
        r.set_rx_rate(20)?;
        Ok(r)
    }

    /// RX channels being computed (tuned, inside the input's range).
    pub fn active_rx(&self) -> usize {
        self.rx.iter().filter(|c| c.ddc.is_some()).count()
    }

    /// The source has ended (file without looping, or a device error).
    pub fn source_ended(&self) -> bool {
        matches!(&self.input, Input::Feed(f) if f.ended())
    }

    fn rx_decimation(&self) -> u64 {
        (RX_R1 / self.m) as u64 * self.r2 as u64
    }

    fn ring_frames(&self) -> usize {
        RING_BYTES / (8 * self.rx.len())
    }

    /// Offset of an NCO word from the input centre, if the channel lies
    /// inside the input band (with a margin for the anti-alias roll-off).
    /// The channel's offset in the input, wrapped into its Nyquist zone
    /// (`None` beyond one rate from either edge).
    fn offset(&self, word: u64) -> Option<f64> {
        let f = nco_freq(word, ADC_HZ_HF);
        let d = f - self.cfg.center_hz;
        let rate = self.cfg.rate_hz;
        (d.abs() <= 1.5 * rate).then(|| d - rate * (d / rate).round())
    }

    fn make_rx_ddc(&self, word: u64) -> Option<Ddc> {
        let d = self.offset(word)?;
        let mut ddc = Ddc::rx(d, self.cfg.rate_hz, RX_R1 / self.m, self.r2 as u32);
        ddc.align(self.rx_k);
        Some(ddc)
    }

    fn rebuild_rx(&mut self, ch: usize) {
        if !self.rx[ch].tuned {
            // an untuned channel costs nothing (it reads zeros)
            self.rx[ch].ddc = None;
            return;
        }
        let word = self.rx[ch].word;
        let keep = self.rx[ch].ddc.is_some() && self.offset(word).is_some();
        if keep {
            // retune in place: keeps filter state and decimation phase
            let d = self.offset(word).unwrap();
            self.rx[ch]
                .ddc
                .as_mut()
                .unwrap()
                .set_offset(d, self.cfg.rate_hz);
        } else {
            self.rx[ch].ddc = self.make_rx_ddc(word);
        }
    }

    /// Run one chunk of input through every active chain, chain by chain
    /// (tight per-channel loops; all RX chains are in step, so each
    /// delivers the same number of outputs per chunk).
    fn process(&mut self, chunk: &[Complex32]) {
        // overflow count and DC removal, once for all chains
        let mut x = std::mem::take(&mut self.pre);
        x.clear();
        let mut clips = 0u32;
        for &raw in chunk {
            if raw.re.abs() >= CLIP || raw.im.abs() >= CLIP {
                clips += 1;
            }
            x.push(if self.cfg.dc_block {
                self.dc += (raw - self.dc) * (1.0 / 16384.0);
                raw - self.dc
            } else {
                raw
            });
        }
        let mut out = std::mem::take(&mut self.out);

        // RX: frames completed in this chunk, zeros for idle channels
        let dmod = self.rx_decimation();
        let frames = ((self.rx_k + x.len() as u64) / dmod - self.rx_k / dmod) as usize;
        let n = self.rx.len();
        let base = self.rx_q.len();
        self.rx_q.resize(base + frames * n, Iq32::default());
        let rx_scale = RX_SINE_FULL_SCALE * self.gain;
        for (c, ch) in self.rx.iter_mut().enumerate() {
            let Some(ddc) = &mut ch.ddc else { continue };
            ddc.process(&x, &mut out);
            debug_assert_eq!(out.len(), frames);
            for (f, y) in out.iter().enumerate() {
                let z = y * rx_scale;
                self.rx_q[base + f * n + c] = Iq32 {
                    i: q32(z.re),
                    q: q32(z.im),
                };
            }
        }
        self.rx_k += x.len() as u64;
        let cap = self.ring_frames() * n;
        if self.rx_q.len() > cap {
            // the hardware ring lapped: oldest frames are gone
            let lost = self.rx_q.len() - cap;
            self.rx_q.drain(..lost);
            self.stats.rx_overruns += 1;
            self.stats.rx_overrun_bytes += 8 * lost as u64;
        }

        // WF
        let wf_scale = WF_SINE_FULL_SCALE * self.gain;
        for s in &mut self.wf {
            if !s.active {
                continue;
            }
            let Some(ddc) = &mut s.ddc else { continue };
            let ring = RING_BYTES / 4;
            if s.mode == WfMode::Continuous && s.q.len() >= ring {
                // the reader is behind (a wide span is read only in part):
                // lose this input uncomputed, as a full ring would, but
                // without spending the DDC on it
                s.skipped = true;
                continue;
            }
            if std::mem::take(&mut s.skipped) {
                s.gaps += 1;
            }
            ddc.process(&x, &mut out);
            for y in &out {
                let z = y * wf_scale;
                s.q.push_back(Iq16 {
                    i: q16(z.re),
                    q: q16(z.im),
                });
                s.produced += 1;
                if s.mode == WfMode::OneShot && s.produced >= WF_BLOCK as u64 {
                    s.active = false;
                    break;
                }
            }
            if s.mode == WfMode::Continuous && s.q.len() > ring {
                let lost = s.q.len() - ring;
                s.q.drain(..lost);
                s.gaps += 1;
            }
        }
        self.pre = x;
        self.out = out;
        self.stats.adc_overflows = self.stats.adc_overflows.wrapping_add(clips);
        self.consumed += chunk.len() as u64;
    }

    /// Take in what the source has produced; in direct mode, read until
    /// `done` says enough.
    fn pump(&mut self, done: impl Fn(&Self) -> bool) -> Result<()> {
        loop {
            if done(self) {
                return Ok(());
            }
            let chunk = match &mut self.input {
                Input::Feed(f) => match f.try_next() {
                    Some(c) => c,
                    None => return Ok(()),
                },
                Input::Direct(src) => {
                    let mut buf = vec![Complex32::default(); 1 << 14];
                    let n = src.read(&mut buf)?;
                    if n == 0 {
                        return Ok(());
                    }
                    buf.truncate(n);
                    buf
                }
            };
            self.process(&chunk);
        }
    }

    /// Seconds of input consumed (Direct) or wall time (Feed).
    fn time(&self) -> f64 {
        match self.input {
            Input::Feed(_) => self.t0.elapsed().as_secs_f64(),
            Input::Direct(_) => self.consumed as f64 / self.cfg.rate_hz,
        }
    }
}

fn q32(x: f64) -> i32 {
    x.round().clamp(i32::MIN as f64, i32::MAX as f64) as i32
}

fn q16(x: f64) -> i16 {
    x.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16
}

impl Radio for IqRadio {
    fn caps(&self) -> Caps {
        Caps {
            variant: Variant::Hf,
            rx_channels: self.rx.len(),
            wf_channels: WF_SLOTS,
            adc_hz: ADC_HZ_HF,
            rx_r2: 5..=40,
            wf_decim_min: self.m as u16,
            wf_decim_max: 8192,
            wf_block: WF_BLOCK,
            sample_counter: true,
            rf_range: (self.cfg.center_hz - 1.5 * self.cfg.rate_hz).max(0.0)
                ..=self.cfg.center_hz + 1.5 * self.cfg.rate_hz,
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
        self.rx_k = 0;
        self.rx_q.clear();
        for ch in 0..self.rx.len() {
            self.rx[ch].ddc = None;
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
        self.rx[ch].tuned = true;
        self.rebuild_rx(ch);
        Ok(())
    }

    fn idle_rx(&mut self, ch: usize) -> Result<()> {
        let c = self.rx.get_mut(ch).ok_or(Error::InvalidChannel(ch))?;
        c.tuned = false;
        c.ddc = None;
        Ok(())
    }

    fn read_rx(&mut self, buf: &mut [Iq32]) -> Result<usize> {
        let n = self.rx.len();
        if !buf.len().is_multiple_of(n) {
            return Err(Error::BadBuffer);
        }
        let want = buf.len();
        self.pump(|r| r.rx_q.len() >= want)?;
        let take = (self.rx_q.len().min(want) / n) * n;
        for (o, s) in buf.iter_mut().zip(self.rx_q.drain(..take)) {
            *o = s;
        }
        Ok(take / n)
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
        if (decim as u32) < self.m {
            return Err(Error::InvalidParam(
                "WF decimation below the IQ input's divisor (wider than the input)",
            ));
        }
        let word = nco_word(f_hz, self.adc_assumed);
        let ddc = self
            .offset(word)
            .map(|d| Ddc::wf(d, self.cfg.rate_hz, decim as u32 / self.m));
        let s = &mut self.wf[slot];
        s.word = word;
        s.decim = decim;
        s.mode = mode;
        s.ddc = ddc;
        s.q.clear();
        s.produced = 0;
        s.active = true;
        s.gaps = 0;
        s.skipped = false;
        Ok(())
    }

    fn wf_gaps(&self, slot: usize) -> u64 {
        self.wf.get(slot).map_or(0, |s| s.gaps)
    }

    fn read_wf(&mut self, slot: usize, buf: &mut [Iq16]) -> Result<usize> {
        if slot >= WF_SLOTS {
            return Err(Error::InvalidChannel(slot));
        }
        let want = buf.len();
        self.pump(|r| {
            let s = &r.wf[slot];
            s.q.len() >= want || !s.active
        })?;
        let s = &mut self.wf[slot];
        if s.ddc.is_none() && s.active {
            // tuned outside the input: zeros at the WF rate
            let n = want.min(match s.mode {
                WfMode::OneShot => WF_BLOCK - s.produced as usize,
                WfMode::Continuous => want,
            });
            buf[..n].fill(Iq16::default());
            s.produced += n as u64;
            if s.mode == WfMode::OneShot && s.produced >= WF_BLOCK as u64 {
                s.active = false;
            }
            return Ok(n);
        }
        let n = s.q.len().min(want);
        for (o, v) in buf.iter_mut().zip(s.q.drain(..n)) {
            *o = v;
        }
        Ok(n)
    }

    fn front_end(&mut self, fe: FrontEnd) -> Result<()> {
        if !(0.0..=31.5).contains(&fe.atten_db) {
            return Err(Error::InvalidParam("attenuation 0..31.5 dB"));
        }
        self.gain = 10f64.powf(-(fe.atten_db as f64) / 20.0);
        Ok(())
    }

    fn pps(&mut self) -> Result<Option<u32>> {
        let secs = self.time() as u64;
        if secs > self.pps_emitted {
            self.pps_emitted = secs;
            Ok(Some(ADC_HZ_HF as u32))
        } else {
            Ok(None)
        }
    }

    fn hw_stats(&mut self) -> Result<Option<HwStats>> {
        let mut s = self.stats;
        if let Input::Feed(f) = &self.input {
            s.rx_frames_dropped = (f.dropped() / self.rx_decimation()) as u32;
        }
        Ok(Some(s))
    }

    fn sample_counter(&mut self) -> Option<u64> {
        Some(self.consumed * self.m as u64)
    }

    fn identity(&self) -> Identity {
        Identity {
            backend: "iq",
            signature: 0x1000_0200 | self.rx.len() as u32,
            dna: None,
            build_id: None,
            live: self.live,
        }
    }
}
