// SPDX-License-Identifier: GPL-3.0-or-later
//! Transport to the driver: one method per driver operation, so the radio
//! logic can be tested against a fake driver on any host.

use crate::abi::*;
use std::io;

pub trait Device: Send {
    /// Scalar ioctl (argument by value).
    fn set(&mut self, cmd: u32, arg: u32) -> io::Result<()>;
    fn get_u32(&mut self, cmd: u32) -> io::Result<u32>;
    fn get_dna(&mut self) -> io::Result<u64>;
    fn rx_param(&mut self, ch: u32, word: u64) -> io::Result<()>;
    fn wf_param(&mut self, ch: u16, decim: u16, word: u64) -> io::Result<()>;
    /// Copy up to `dst.len()` bytes of RX ring data; may be short or 0, and
    /// need not end on a frame boundary.
    fn rx_read(&mut self, dst: &mut [u8]) -> io::Result<usize>;
    fn wf_read(&mut self, ch: u16, dst: &mut [u8]) -> io::Result<usize>;
    /// Next PPS period, `None` if there's none new.
    fn pps_read(&mut self) -> io::Result<Option<u32>>;
    /// Open driver's counters; `None` with the stock driver.
    fn stats(&mut self) -> io::Result<Option<Stats>> {
        Ok(None)
    }

    /// The channelizer bitstream (open driver; unsupported elsewhere).
    fn pfb_info(&mut self) -> io::Result<PfbInfo> {
        Err(io::ErrorKind::Unsupported.into())
    }
    fn pfb_set_mask(&mut self, _generation: u32, _mask: &[u32; 64]) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }
    fn pfb_start(&mut self, _streams: u32, _power_frames: u32) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }
    /// Ring bytes of `stream` (`PFB_BINS` or `PFB_POWER`) since the last
    /// read; may be short or 0.
    fn pfb_read(&mut self, _stream: u32, _dst: &mut [u8]) -> io::Result<usize> {
        Err(io::ErrorKind::Unsupported.into())
    }
    fn pfb_status(&mut self) -> io::Result<PfbStatus> {
        Err(io::ErrorKind::Unsupported.into())
    }
}

/// The real `/dev/zynqsdr`. Only works on 32-bit ARM: the driver takes user
/// buffer addresses as u32.
pub struct ZynqsdrDev {
    fd: std::os::fd::OwnedFd,
}

impl ZynqsdrDev {
    pub const PATH: &'static str = "/dev/zynqsdr";

    pub fn open(path: &str) -> io::Result<Self> {
        if cfg!(not(target_pointer_width = "32")) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "the zynqsdr ABI passes 32-bit user addresses; run on the device",
            ));
        }
        use std::os::unix::fs::OpenOptionsExt;
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_SYNC | libc::O_CLOEXEC)
            .open(path)?;
        Ok(ZynqsdrDev { fd: f.into() })
    }

    fn raw(&self) -> libc::c_int {
        use std::os::fd::AsRawFd;
        self.fd.as_raw_fd()
    }

    fn ioctl(&self, cmd: u32, arg: libc::c_ulong) -> io::Result<libc::c_int> {
        // SAFETY: cmd/arg follow the driver ABI; pointer args reference live,
        // correctly sized structs owned by the caller for the call's duration.
        let r = unsafe { libc::ioctl(self.raw(), cmd as _, arg) };
        if r < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(r)
        }
    }

    fn ioctl_ptr<T>(&self, cmd: u32, arg: &mut T) -> io::Result<()> {
        debug_assert_eq!(std::mem::size_of::<T>(), ioc_size(cmd));
        self.ioctl(cmd, arg as *mut T as usize as libc::c_ulong)
            .map(|_| ())
    }
}

fn addr32(p: *mut u8) -> io::Result<u32> {
    u32::try_from(p as usize).map_err(|_| io::Error::other("buffer address above 4 GiB"))
}

impl Device for ZynqsdrDev {
    fn set(&mut self, cmd: u32, arg: u32) -> io::Result<()> {
        self.ioctl(cmd, arg as libc::c_ulong).map(|_| ())
    }

    fn get_u32(&mut self, cmd: u32) -> io::Result<u32> {
        let mut v = 0u32;
        self.ioctl_ptr(cmd, &mut v)?;
        Ok(v)
    }

    fn get_dna(&mut self) -> io::Result<u64> {
        let mut v = 0u64;
        self.ioctl_ptr(GET_DNA, &mut v)?;
        Ok(v)
    }

    fn rx_param(&mut self, ch: u32, word: u64) -> io::Result<()> {
        let mut p = RxParam {
            channel: ch,
            freq: word,
        };
        self.ioctl_ptr(RX_PARAM, &mut p)
    }

    fn wf_param(&mut self, ch: u16, decim: u16, word: u64) -> io::Result<()> {
        let mut p = WfParam {
            channel: ch,
            decimate: decim,
            freq: word,
        };
        self.ioctl_ptr(WF_PARAM, &mut p)
    }

    fn rx_read(&mut self, dst: &mut [u8]) -> io::Result<usize> {
        let mut r = RxRead {
            address: addr32(dst.as_mut_ptr())?,
            length: dst.len() as u32,
            readed: 0,
        };
        self.ioctl_ptr(RX_READ, &mut r)?;
        Ok((r.readed as usize).min(dst.len()))
    }

    fn wf_read(&mut self, ch: u16, dst: &mut [u8]) -> io::Result<usize> {
        let mut r = WfRead {
            channel: ch,
            address: addr32(dst.as_mut_ptr())?,
            length: dst.len() as u32,
            readed: 0,
        };
        self.ioctl_ptr(WF_READ, &mut r)?;
        Ok((r.readed as usize).min(dst.len()))
    }

    fn pps_read(&mut self) -> io::Result<Option<u32>> {
        match self.get_u32(PPS_READ) {
            Ok(v) => Ok(Some(v)),
            Err(e) if e.raw_os_error() == Some(libc::EBUSY) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn pfb_info(&mut self) -> io::Result<PfbInfo> {
        let mut v = PfbInfo::default();
        self.ioctl_ptr(PFB_INFO, &mut v)?;
        Ok(v)
    }

    fn pfb_set_mask(&mut self, generation: u32, mask: &[u32; 64]) -> io::Result<()> {
        let mut m = PfbMask {
            generation,
            mask: *mask,
        };
        self.ioctl_ptr(PFB_SET_MASK, &mut m)
    }

    fn pfb_start(&mut self, streams: u32, power_frames: u32) -> io::Result<()> {
        let mut s = PfbStart {
            streams,
            power_frames,
        };
        self.ioctl_ptr(PFB_START, &mut s)
    }

    fn pfb_read(&mut self, stream: u32, dst: &mut [u8]) -> io::Result<usize> {
        let mut r = PfbRead {
            stream,
            address: addr32(dst.as_mut_ptr())?,
            length: dst.len() as u32,
            readed: 0,
        };
        self.ioctl_ptr(PFB_READ, &mut r)?;
        Ok((r.readed as usize).min(dst.len()))
    }

    fn pfb_status(&mut self) -> io::Result<PfbStatus> {
        let mut s = PfbStatus::default();
        self.ioctl_ptr(PFB_STATUS, &mut s)?;
        Ok(s)
    }

    fn stats(&mut self) -> io::Result<Option<Stats>> {
        let mut s = Stats::default();
        match self.ioctl_ptr(GET_STATS, &mut s) {
            Ok(()) => Ok(Some(s)),
            // stock driver: unknown ioctl
            Err(e) if matches!(e.raw_os_error(), Some(libc::ENOTTY | libc::EINVAL)) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
