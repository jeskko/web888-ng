// SPDX-License-Identifier: GPL-3.0-or-later
//! Hardware abstraction for the Web-888.
//!
//! The [`Radio`] trait is the only way the rest of web888d touches hardware.
//! Backends: `Sim` (crate `web888-sim`), later `ZynqsdrIoctl` (stock driver)
//! and the open driver. The data formats and channel math here follow the
//! FPGA contract in web888-ng-hw (`fpga/doc/register-map.md`,
//! `docs/stock-driver.md`).

use std::fmt;

pub mod chan;
pub mod clock;

pub use chan::*;

/// One RX sample of one channel, exactly as in the DMA ring
/// (little endian, I at the lower address). Full scale: see [`RX_DC_FULL_SCALE`].
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Iq32 {
    pub i: i32,
    pub q: i32,
}

/// One waterfall sample, as in the WF ring (I in bits 15:0, Q in 31:16).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Iq16 {
    pub i: i16,
    pub q: i16,
}

/// Which bitstream / ADC clock family is running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// 122.88 MHz ADC clock, 0..61 MHz (stock: 12 RX channels).
    Hf,
    /// 61.44..110.592 MHz ADC clock (narrowband / airband; stock: 13 RX).
    Vhf,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Caps {
    pub variant: Variant,
    pub rx_channels: usize,
    pub wf_channels: usize,
    /// Nominal ADC sample rate in Hz (uncorrected).
    pub adc_hz: f64,
    /// Accepted stage-2 RX decimation range (total decimation = 256 * r2).
    pub rx_r2: std::ops::RangeInclusive<u16>,
    /// Largest WF decimation (powers of two from 1).
    pub wf_decim_max: u16,
    /// WF block length in one-shot mode.
    pub wf_block: usize,
    /// Open FPGA: 64-bit sample counter available.
    pub sample_counter: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WfMode {
    /// Capture one block of `Caps::wf_block` samples after the start, then stop.
    OneShot,
    /// Stream continuously.
    Continuous,
}

/// Analogue front end and GPIO state.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrontEnd {
    /// RF attenuator, 0..=31.5 dB in 0.5 dB steps.
    pub atten_db: f32,
    pub pga: bool,
    pub dither: bool,
    /// Antenna switch outputs (6 bits).
    pub antenna: u8,
    /// HF/VHF RF path switch (true = VHF/airband path).
    pub vhf_path: bool,
    /// External reference clock select.
    pub ext_ref: bool,
    pub led: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// Backend name, e.g. "sim", "zynqsdr".
    pub backend: &'static str,
    /// FPGA signature word (STS 0x00).
    pub signature: u32,
    /// 57-bit device DNA, if the backend has one.
    pub dna: Option<u64>,
    /// Open FPGA build id (yyyymmdd as hex digits).
    pub build_id: Option<u32>,
}

#[derive(Debug)]
pub enum Error {
    InvalidChannel(usize),
    InvalidParam(&'static str),
    /// Buffer length is not a whole number of RX frames.
    BadBuffer,
    Unsupported(&'static str),
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidChannel(c) => write!(f, "invalid channel {c}"),
            Error::InvalidParam(p) => write!(f, "invalid parameter: {p}"),
            Error::BadBuffer => write!(f, "buffer is not a whole number of RX frames"),
            Error::Unsupported(w) => write!(f, "unsupported: {w}"),
            Error::Io(e) => write!(f, "I/O: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Hardware backend. Owned by the hw thread; all calls are non-blocking.
pub trait Radio: Send {
    fn caps(&self) -> Caps;

    /// ADC sample rate currently used for NCO words (nominal or PPS-corrected).
    fn adc_hz(&self) -> f64;
    /// Set the PPS-corrected ADC rate; NCO words are recomputed on the next tune.
    fn set_adc_hz(&mut self, hz: f64);

    /// Set the stage-2 RX decimation (all channels share one output rate);
    /// restarts the RX stream.
    fn set_rx_rate(&mut self, r2: u16) -> Result<()>;
    fn rx_r2(&self) -> u16;
    fn tune_rx(&mut self, ch: usize, f_hz: f64) -> Result<()>;

    /// Read whole RX frames (`caps().rx_channels` samples each, channel 0
    /// first) into `buf`. Returns the number of frames; 0 if none are ready.
    fn read_rx(&mut self, buf: &mut [Iq32]) -> Result<usize>;

    fn start_wf(&mut self, slot: usize, f_hz: f64, decim: u16, mode: WfMode) -> Result<()>;
    /// Read WF samples; returns the count (0 if none ready, or one-shot done).
    fn read_wf(&mut self, slot: usize, buf: &mut [Iq16]) -> Result<usize>;

    fn front_end(&mut self, fe: FrontEnd) -> Result<()>;

    /// Next PPS period in ADC clocks, if a new one arrived.
    fn pps(&mut self) -> Result<Option<u32>>;
    /// Open FPGA only: ADC samples since reset.
    fn sample_counter(&mut self) -> Option<u64>;

    fn identity(&self) -> Identity;
}
