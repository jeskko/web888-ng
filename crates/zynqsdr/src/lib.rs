// SPDX-License-Identifier: GPL-3.0-or-later
//! [`Radio`] backend for the stock Web-888 kernel driver (`/dev/zynqsdr`).
//! Works with the stock and the open bitstream on unmodified firmware.
//! Written from web888-ng-hw `docs/stock-driver.md` and
//! `docs/server-fpga-interface.md` only (clean-room rule, design §12).
//!
//! Notes:
//! * The driver allows one opener: the stock server must not be running.
//! * RX_READ/WF_READ may return partial frames; whole frames are assembled here.
//! * Waterfall: one-shot is a plain `WF_START` (like the stock server);
//!   continuous adds `WF_READ_CONTINUES`. One-shot reads stop after one
//!   block. The stock driver ignores the flag (a bug: it always selects
//!   one-shot), so without the open driver continuous is emulated: the
//!   next block is started as soon as one is read, with a gap between
//!   blocks. A continuous stream that stalls anyway is restarted.
//! * The channelizer bitstream (signature 0x0E5D0010, open driver): no RX
//!   or WF channels; the bins and the power line through [`BinRadio`].

pub mod abi;
pub mod bitstream;
pub mod dev;
pub mod pfb;
pub mod si5351;

use abi::*;
use dev::Device;
use std::sync::Arc;
use std::time::{Duration, Instant};
use web888_radio::bins::{BinCaps, BinFrame, BinRadio, selection};
use web888_radio::*;

/// Static facts the driver can't tell us.
#[derive(Clone, Debug)]
pub struct ZynqConfig {
    pub variant: Variant,
    /// Nominal ADC clock as programmed (Si5351 / FSBL).
    pub adc_hz: f64,
}

impl Default for ZynqConfig {
    fn default() -> Self {
        ZynqConfig {
            variant: Variant::Hf,
            adc_hz: ADC_HZ_HF,
        }
    }
}

struct WfState {
    decim: u16,
    mode: WfMode,
    word: u64,
    produced: u64,
    last_data: Instant,
    active: bool,
    stage: Vec<u8>,
    /// Continuous-mode restarts since `start_wf` (`Radio::wf_gaps`).
    gaps: u64,
}

pub struct ZynqsdrRadio<D: Device> {
    dev: D,
    cfg: ZynqConfig,
    signature: u32,
    dna: u64,
    rx_ch: usize,
    wf_ch: usize,
    adc_assumed: f64,
    r2: u16,
    words: Vec<u64>,
    rx_stage: Vec<u8>,
    wf: Vec<WfState>,
    gpio: u32,
    /// From GET_STATS at open (open driver + open bitstream).
    build_id: Option<u32>,
    /// Open driver: GET_STATS works, and `WF_READ_CONTINUES` is honoured.
    has_stats: bool,
    /// The channelizer bitstream.
    pfb: Option<Pfb>,
}

/// The channelizer's state (`BinRadio`).
struct Pfb {
    generation: u8,
    streams: u32,
    power_frames: u16,
    bins: pfb::BinsParser,
    power: pfb::PowerParser,
    buf: Vec<u8>,
}

/// The signature's channelizer field ([7:4] = 1, open bitstream).
pub fn is_channelizer(signature: u32) -> bool {
    signature >> 16 == 0x0e5d && (signature >> 4) & 0xf == 1
}

fn io(e: std::io::Error) -> Error {
    Error::Io(e)
}

impl<D: Device> ZynqsdrRadio<D> {
    /// Identify the bitstream, enable PPS and start RX at 24 kS/s.
    pub fn new(mut dev: D, cfg: ZynqConfig) -> Result<Self> {
        let signature = dev.get_u32(GET_SIGNATURE).map_err(io)?;
        let (rx_ch, wf_ch) = signature_counts(signature);
        let channelizer = is_channelizer(signature);
        if !channelizer && (rx_ch == 0 || rx_ch > 15 || wf_ch == 0) {
            return Err(Error::Unsupported(
                "FPGA signature reports no channels (bitstream loaded?)",
            ));
        }
        let dna = dev.get_dna().map_err(io)?;
        let gpio = dev.get_u32(GET_GPIO_MASK).map_err(io)?;
        let stats = dev.stats().map_err(io)?;
        dev.set(PPS_START, 1).map_err(io)?;
        let adc = cfg.adc_hz;
        let mut r = ZynqsdrRadio {
            dev,
            cfg,
            signature,
            dna,
            rx_ch,
            wf_ch,
            adc_assumed: adc,
            r2: 20,
            words: vec![0; rx_ch],
            rx_stage: Vec::new(),
            wf: (0..wf_ch)
                .map(|_| WfState {
                    decim: 1,
                    mode: WfMode::OneShot,
                    word: 0,
                    produced: 0,
                    last_data: Instant::now(),
                    active: false,
                    stage: Vec::new(),
                    gaps: 0,
                })
                .collect(),
            gpio,
            build_id: stats.filter(|s| s.ext != 0).map(|s| s.build_id),
            has_stats: stats.is_some(),
            pfb: channelizer.then(|| Pfb {
                generation: 0,
                streams: 0,
                power_frames: 0,
                bins: pfb::BinsParser::default(),
                power: pfb::PowerParser::new(1),
                buf: vec![0; 256 * 1024],
            }),
        };
        if channelizer {
            return Ok(r);
        }
        r.set_rx_rate(20)?;
        Ok(r)
    }

    pub fn device(&mut self) -> &mut D {
        &mut self.dev
    }

    fn wf_restart(&mut self, slot: usize) -> Result<()> {
        let s = &mut self.wf[slot];
        self.dev
            .wf_param(slot as u16, s.decim, s.word)
            .map_err(io)?;
        let flag = match s.mode {
            WfMode::Continuous => WF_READ_CONTINUES,
            WfMode::OneShot => 0,
        };
        self.dev.set(WF_START, slot as u32 | flag).map_err(io)?;
        s.produced = 0;
        s.stage.clear();
        s.last_data = Instant::now();
        s.active = true;
        Ok(())
    }

    /// Stall time after which a continuous waterfall is restarted.
    fn wf_stall_limit(&self, slot: usize) -> Duration {
        let block = WF_BLOCK as f64 * self.wf[slot].decim as f64 / self.adc_assumed;
        Duration::from_secs_f64((3.0 * block).max(0.05))
    }
}

impl<D: Device> Radio for ZynqsdrRadio<D> {
    fn caps(&self) -> Caps {
        Caps {
            variant: self.cfg.variant,
            rx_channels: self.rx_ch,
            wf_channels: self.wf_ch,
            adc_hz: self.cfg.adc_hz,
            rx_r2: 5..=40,
            wf_decim_min: 1,
            wf_decim_max: 8192,
            wf_block: WF_BLOCK,
            sample_counter: false,
            // the VHF bitstream's higher zones (undersampling) come later
            rf_range: 0.0..=self.cfg.adc_hz / 2.0,
        }
    }

    fn adc_hz(&self) -> f64 {
        self.adc_assumed
    }

    fn set_adc_hz(&mut self, hz: f64) {
        self.adc_assumed = hz;
    }

    fn set_rx_rate(&mut self, r2: u16) -> Result<()> {
        if self.pfb.is_some() {
            return Err(Error::Unsupported(
                "the channelizer bitstream has no RX channels",
            ));
        }
        if !(5..=40).contains(&r2) {
            return Err(Error::InvalidParam("r2 out of range"));
        }
        self.dev.set(RX_START, r2 as u32).map_err(io)?;
        self.r2 = r2;
        // the ring restarts at offset 0 with frame 0
        self.rx_stage.clear();
        Ok(())
    }

    fn rx_r2(&self) -> u16 {
        self.r2
    }

    fn tune_rx(&mut self, ch: usize, f_hz: f64) -> Result<()> {
        if ch >= self.rx_ch {
            return Err(Error::InvalidChannel(ch));
        }
        let w = nco_word(f_hz, self.adc_assumed);
        self.dev.rx_param(ch as u32, w).map_err(io)?;
        self.words[ch] = w;
        Ok(())
    }

    fn read_rx(&mut self, buf: &mut [Iq32]) -> Result<usize> {
        let n = self.rx_ch;
        if n == 0 {
            return Err(Error::Unsupported(
                "the channelizer bitstream has no RX channels",
            ));
        }
        if !buf.len().is_multiple_of(n) {
            return Err(Error::BadBuffer);
        }
        let frame_bytes = 8 * n;
        let want = buf.len() * 8;
        let have = self.rx_stage.len();
        if have < want {
            self.rx_stage.resize(want, 0);
            let got = self.dev.rx_read(&mut self.rx_stage[have..]).map_err(io)?;
            self.rx_stage.truncate(have + got);
        }
        let frames = (self.rx_stage.len() / frame_bytes).min(buf.len() / n);
        let bytes = frames * frame_bytes;
        for (k, c) in self.rx_stage[..bytes].as_chunks::<8>().0.iter().enumerate() {
            buf[k] = Iq32 {
                i: i32::from_le_bytes(c[0..4].try_into().unwrap()),
                q: i32::from_le_bytes(c[4..8].try_into().unwrap()),
            };
        }
        self.rx_stage.drain(..bytes);
        Ok(frames)
    }

    fn start_wf(&mut self, slot: usize, f_hz: f64, decim: u16, mode: WfMode) -> Result<()> {
        if slot >= self.wf_ch {
            return Err(Error::InvalidChannel(slot));
        }
        if decim == 0 || decim > 8192 || !decim.is_power_of_two() {
            return Err(Error::InvalidParam(
                "WF decimation must be a power of two 1..=8192",
            ));
        }
        let word = nco_word(f_hz, self.adc_assumed);
        let s = &mut self.wf[slot];
        s.decim = decim;
        s.mode = mode;
        s.word = word;
        s.gaps = 0;
        self.wf_restart(slot)
    }

    fn wf_gaps(&self, slot: usize) -> u64 {
        self.wf.get(slot).map_or(0, |s| s.gaps)
    }

    fn read_wf(&mut self, slot: usize, buf: &mut [Iq16]) -> Result<usize> {
        if slot >= self.wf_ch {
            return Err(Error::InvalidChannel(slot));
        }
        if !self.wf[slot].active {
            return Ok(0);
        }
        let mut want = buf.len();
        if self.wf[slot].mode == WfMode::OneShot {
            want = want.min(WF_BLOCK - self.wf[slot].produced as usize);
        }
        let s = &mut self.wf[slot];
        let have = s.stage.len();
        if have < want * 4 {
            s.stage.resize(want * 4, 0);
            let got = self
                .dev
                .wf_read(slot as u16, &mut s.stage[have..])
                .map_err(io)?;
            s.stage.truncate(have + got);
            if got > 0 {
                s.last_data = Instant::now();
            }
        }
        let n = (s.stage.len() / 4).min(want);
        for (k, c) in s.stage[..n * 4].as_chunks::<4>().0.iter().enumerate() {
            buf[k] = Iq16 {
                i: i16::from_le_bytes([c[0], c[1]]),
                q: i16::from_le_bytes([c[2], c[3]]),
            };
        }
        s.stage.drain(..n * 4);
        s.produced += n as u64;
        let block_done = s.produced >= WF_BLOCK as u64;
        if s.mode == WfMode::OneShot && block_done {
            s.active = false;
        }
        if self.wf[slot].mode == WfMode::Continuous
            && ((!self.has_stats && block_done && self.wf[slot].stage.is_empty())
                || (n == 0 && self.wf[slot].last_data.elapsed() > self.wf_stall_limit(slot)))
        {
            // stock driver: one-shot only, start the next block now
            self.wf_restart(slot)?;
            self.wf[slot].gaps += 1;
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
        self.dev
            .set(AD8370_SET, atten_code(fe.atten_db))
            .map_err(io)?;
        self.dev.set(MODE_SET, fe.vhf_path as u32).map_err(io)?;
        self.dev.set(CLK_SET, (!fe.ext_ref) as u32).map_err(io)?;
        let mut g = self.gpio & !(GPIO_ANTENNA_MASK | GPIO_DITHER | GPIO_PGA | GPIO_LED);
        g |= fe.antenna as u32 & GPIO_ANTENNA_MASK;
        if fe.dither {
            g |= GPIO_DITHER;
        }
        if fe.pga {
            g |= GPIO_PGA;
        }
        if fe.led {
            g |= GPIO_LED;
        }
        self.dev.set(SET_GPIO_MASK, g).map_err(io)?;
        self.gpio = g;
        Ok(())
    }

    fn pps(&mut self) -> Result<Option<u32>> {
        self.dev.pps_read().map_err(io)
    }

    fn hw_stats(&mut self) -> Result<Option<HwStats>> {
        if !self.has_stats {
            return Ok(None);
        }
        Ok(self.dev.stats().map_err(io)?.map(|s| HwStats {
            adc_overflows: s.adc_overflows,
            rx_frames_dropped: s.rx_frames_dropped,
            rx_overruns: s.rx_overruns,
            rx_overrun_bytes: s.rx_overrun_bytes,
        }))
    }

    fn sample_counter(&mut self) -> Option<u64> {
        None
    }

    fn identity(&self) -> Identity {
        Identity {
            backend: "zynqsdr",
            signature: self.signature,
            dna: Some(self.dna),
            build_id: self.build_id,
            live: true,
        }
    }
}

/// The channelizer bitstream; with any other every call fails.
impl<D: Device> BinRadio for ZynqsdrRadio<D> {
    fn bin_caps(&self) -> BinCaps {
        let on = self.pfb.is_some();
        BinCaps {
            bin_hz: self.adc_assumed / 4096.0,
            frame_rate: self.adc_assumed / 2048.0,
            // bin 2047's slot carries the next frame's header
            bins: if on { pfb::POWER_BINS - 1 } else { 0 },
            first_hz: 0.0,
            wraps: false,
            max_selected: if on { 64 } else { 0 },
        }
    }

    fn set_bins(&mut self, bins: &[usize]) -> Result<u8> {
        let caps = self.bin_caps();
        let p = pfb_mut(&mut self.pfb)?;
        let sel = selection(bins, caps.bins, caps.max_selected);
        let mut mask = [0u32; 64];
        for &b in &sel {
            mask[b / 32] |= 1 << (b % 32);
        }
        // 1..=15, never the one in use: a new one switches the banks
        let generation = p.generation % 15 + 1;
        self.dev
            .pfb_set_mask(generation as u32, &mask)
            .map_err(io)?;
        p.bins.set_selection(generation, Arc::from(sel));
        p.generation = generation;
        if p.streams & PFB_BINS == 0 {
            p.streams |= PFB_BINS;
            self.dev
                .pfb_start(p.streams, p.power_frames.max(1) as u32)
                .map_err(io)?;
        }
        Ok(generation)
    }

    fn read_bins(&mut self, out: &mut Vec<BinFrame>) -> Result<usize> {
        let p = pfb_mut(&mut self.pfb)?;
        if p.streams & PFB_BINS == 0 {
            return Ok(0);
        }
        let mut n = 0;
        loop {
            let got = self.dev.pfb_read(PFB_BINS, &mut p.buf).map_err(io)?;
            n += p.bins.push(&p.buf[..got], out);
            if got < p.buf.len() {
                return Ok(n);
            }
        }
    }

    fn set_power(&mut self, frames: u16) -> Result<()> {
        let p = pfb_mut(&mut self.pfb)?;
        let streams = if frames == 0 {
            p.streams & !PFB_POWER
        } else {
            p.streams | PFB_POWER
        };
        self.dev
            .pfb_start(streams, frames.max(1) as u32)
            .map_err(io)?;
        if frames != p.power_frames || streams & PFB_POWER == 0 {
            p.power = pfb::PowerParser::new(frames);
        }
        p.streams = streams;
        p.power_frames = frames;
        Ok(())
    }

    fn read_power(&mut self, line: &mut Vec<f32>) -> Result<Option<u32>> {
        let p = pfb_mut(&mut self.pfb)?;
        if p.streams & PFB_POWER != 0 {
            loop {
                let got = self.dev.pfb_read(PFB_POWER, &mut p.buf).map_err(io)?;
                p.power.push(&p.buf[..got]);
                if got < p.buf.len() {
                    break;
                }
            }
        }
        Ok(p.power.pop().map(|(n, l)| {
            *line = l;
            n
        }))
    }
}

fn pfb_mut(p: &mut Option<Pfb>) -> Result<&mut Pfb> {
    p.as_mut()
        .ok_or(Error::Unsupported("not the channelizer bitstream"))
}
