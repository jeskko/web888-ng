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

/// `WF_START` flag. The driver clears the WF mode bit when it is set and
/// sets it otherwise. Which mode bit value the stock bitstream treats as
/// continuous is not confirmed (the open bitstream streams continuously
/// with the bit set), so web888d never passes this flag (see `ZynqsdrRadio`).
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
    }

    #[test]
    fn attenuator() {
        assert_eq!(atten_code(0.0), 0);
        assert_eq!(atten_code(10.5), 21);
        assert_eq!(atten_code(31.5), 63);
        assert_eq!(atten_code(99.0), 63);
    }
}
