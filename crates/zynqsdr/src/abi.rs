// SPDX-License-Identifier: GPL-3.0-or-later
//! The stock `/dev/zynqsdr` ioctl ABI (type 'Z', 32-bit ARM), from
//! web888-ng-hw `docs/stock-driver.md` and `docs/server-fpga-interface.md`.

/// Scalar ioctls take their argument by value.
pub const AD8370_SET: u32 = 0x4004_5A00; // attenuator code (0.5 dB steps)
pub const MODE_SET: u32 = 0x4004_5A01; // 0 = HF, 1 = VHF/airband RF path
pub const CLK_SET: u32 = 0x4004_5A02; // 0 = external ADC clock, 1 = internal
pub const GET_DNA: u32 = 0x8008_5A03; // -> u64
pub const RX_START: u32 = 0x4004_5A04; // stage-2 decimation (5..40)
pub const RX_PARAM: u32 = 0x400C_5A05; // RxParam
pub const RX_READ: u32 = 0xC00C_5A06; // RxRead
pub const SET_GPIO_MASK: u32 = 0x4004_5A07;
pub const GET_GPIO_MASK: u32 = 0x8004_5A08; // -> u32
pub const GET_SIGNATURE: u32 = 0x8004_5A09; // -> u32
pub const WF_START: u32 = 0x4004_5A0A; // channel (bit16: see WF_READ_CONTINUES)
pub const WF_PARAM: u32 = 0x400C_5A0B; // WfParam
pub const WF_READ: u32 = 0xC00E_5A0C; // WfRead
pub const PPS_START: u32 = 0x4004_5A14; // 1 = enable
pub const PPS_READ: u32 = 0x8004_5A15; // -> u32, EBUSY if none new

/// Open driver (web888-ng-hw `driver/web888_sdr.h`) only; ENOTTY on stock.
pub const GET_STATS: u32 = 0x8040_5A20; // -> Stats

/// The channelizer bitstream (signature 0x0E5D0010) through the open
/// driver: ENODEV with any other bitstream, ENOTTY with the stock driver.
pub const PFB_INFO: u32 = 0x801C_5A21; // -> PfbInfo
pub const PFB_SET_MASK: u32 = 0x4104_5A22; // PfbMask
pub const PFB_START: u32 = 0x4008_5A23; // PfbStart
pub const PFB_READ: u32 = 0xC010_5A24; // PfbRead
pub const PFB_STATUS: u32 = 0x801C_5A25; // -> PfbStatus

/// `PfbStart::streams`, `PfbRead::stream`.
pub const PFB_BINS: u32 = 1 << 0;
pub const PFB_POWER: u32 = 1 << 1;

/// `WF_START` flag: continuous waterfall. The driver clears the WF mode bit
/// (CFG 0x9E) when it is set and sets it otherwise; mode bit 1 = one-shot
/// (open bitstream; stock by the flag's name, unconfirmed on hardware).
pub const WF_READ_CONTINUES: u32 = 1 << 16;

/// GPIO word bits (`SET/GET_GPIO_MASK`).
pub const GPIO_ANTENNA_MASK: u32 = 0x3f;
pub const GPIO_DITHER: u32 = 1 << 6;
pub const GPIO_PGA: u32 = 1 << 7;
pub const GPIO_LED: u32 = 1 << 8;

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RxParam {
    pub channel: u32,
    /// 48-bit NCO phase increment.
    pub freq: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RxRead {
    /// User virtual address of the destination buffer (32-bit ARM).
    pub address: u32,
    pub length: u32,
    /// Bytes copied (written by the driver).
    pub readed: u32,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct WfParam {
    pub channel: u16,
    pub decimate: u16,
    pub freq: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct WfRead {
    pub channel: u16,
    pub address: u32,
    pub length: u32,
    pub readed: u32,
}

/// `GET_STATS` result (natural alignment, 64 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub size: u32,
    pub signature: u32,
    /// 1 if the FPGA fields are valid (open bitstream).
    pub ext: u32,
    pub build_id: u32,
    pub flags: u32,
    pub adc_overflows: u32,
    pub adc_peak: u32,
    pub rx_frames: u32,
    pub adc_samples: u64,
    pub rx_frames_dropped: u32,
    pub rx_bursts: u32,
    pub wf_bursts: u32,
    pub rx_overruns: u32,
    pub rx_overrun_bytes: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PfbInfo {
    pub size: u32,
    pub bins: u32,
    pub max_selected: u32,
    pub bin_hz: u32,
    pub frame_hz: u32,
    pub bins_ring_bytes: u32,
    pub power_ring_bytes: u32,
}

/// Bit b of `mask[w]` selects bin 32w + b; frames using it carry
/// `generation` (1..=15).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PfbMask {
    pub generation: u32,
    pub mask: [u32; 64],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PfbStart {
    pub streams: u32,
    pub power_frames: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PfbRead {
    pub stream: u32,
    pub address: u32,
    pub length: u32,
    pub readed: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PfbStatus {
    pub size: u32,
    pub generation: u32,
    pub flags: u32,
    pub bins_frames: u32,
    pub bins_dropped: u32,
    pub power_chunks: u32,
    pub power_dropped: u32,
}

/// The size field of an ioctl number (bits 29:16).
pub const fn ioc_size(cmd: u32) -> usize {
    ((cmd >> 16) & 0x3fff) as usize
}

/// Attenuator code for `atten_db` (0..=31.5 dB, 0.5 dB steps).
pub fn atten_code(atten_db: f32) -> u32 {
    (atten_db * 2.0).round().clamp(0.0, 63.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    #[test]
    fn struct_sizes_match_ioctl_numbers() {
        assert_eq!(size_of::<RxParam>(), 12);
        assert_eq!(ioc_size(RX_PARAM), 12);
        assert_eq!(size_of::<RxRead>(), 12);
        assert_eq!(ioc_size(RX_READ), 12);
        assert_eq!(size_of::<WfParam>(), 12);
        assert_eq!(ioc_size(WF_PARAM), 12);
        assert_eq!(size_of::<WfRead>(), 14);
        assert_eq!(ioc_size(WF_READ), 14);
        assert_eq!(ioc_size(GET_DNA), 8);
        assert_eq!(ioc_size(GET_SIGNATURE), 4);
        assert_eq!(size_of::<Stats>(), 64);
        assert_eq!(ioc_size(GET_STATS), 64);
        assert_eq!(std::mem::offset_of!(Stats, adc_samples), 32);
        assert_eq!(std::mem::offset_of!(Stats, rx_overrun_bytes), 56);
        assert_eq!(size_of::<PfbInfo>(), ioc_size(PFB_INFO));
        assert_eq!(size_of::<PfbMask>(), ioc_size(PFB_SET_MASK));
        assert_eq!(size_of::<PfbStart>(), ioc_size(PFB_START));
        assert_eq!(size_of::<PfbRead>(), ioc_size(PFB_READ));
        assert_eq!(size_of::<PfbStatus>(), ioc_size(PFB_STATUS));
        // _IOR/_IOW/_IOWR('Z', nr, size): direction, size, type, number
        for (cmd, dir, nr) in [
            (PFB_INFO, 2, 0x21),
            (PFB_SET_MASK, 1, 0x22),
            (PFB_START, 1, 0x23),
            (PFB_READ, 3, 0x24),
            (PFB_STATUS, 2, 0x25),
        ] {
            assert_eq!((cmd >> 30, (cmd >> 8) & 0xff, cmd & 0xff), (dir, 0x5a, nr));
        }
    }

    #[test]
    fn attenuator() {
        assert_eq!(atten_code(0.0), 0);
        assert_eq!(atten_code(10.5), 21);
        assert_eq!(atten_code(31.5), 63);
        assert_eq!(atten_code(99.0), 63);
    }
}
