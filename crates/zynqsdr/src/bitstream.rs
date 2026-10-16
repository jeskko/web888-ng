// SPDX-License-Identifier: GPL-3.0-or-later
//! Bitstream loading through the legacy Xilinx devcfg device, as the stock
//! system does: the ADC clock must already run (Si5351), and `/dev/zynqsdr`
//! is opened only after the load.

use std::io;
use std::path::{Path, PathBuf};
use web888_radio::Variant;

pub const XDEVCFG: &str = "/dev/xdevcfg";
/// SD card FAT partition on the stock image.
pub const SD_DIR: &str = "/media/mmcblk0p1";

/// Stock file name for a variant: `websdr_hf.bit` / `websdr_vhf.bit`.
pub fn default_path(variant: Variant) -> PathBuf {
    let name = match variant {
        Variant::Hf => "websdr_hf.bit",
        Variant::Vhf => "websdr_vhf.bit",
    };
    Path::new(SD_DIR).join(name)
}

/// Copy `bit` into the devcfg device (`dev`, normally [`XDEVCFG`]).
pub fn load(bit: &Path, dev: &Path) -> io::Result<u64> {
    let mut src = std::fs::File::open(bit)?;
    let mut dst = std::fs::OpenOptions::new().write(true).open(dev)?;
    let n = io::copy(&mut src, &mut dst)?;
    dst.sync_all().ok();
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_names() {
        assert_eq!(
            default_path(Variant::Hf),
            Path::new("/media/mmcblk0p1/websdr_hf.bit")
        );
        assert_eq!(
            default_path(Variant::Vhf),
            Path::new("/media/mmcblk0p1/websdr_vhf.bit")
        );
    }

    #[test]
    fn copies_to_device() {
        let d = std::env::temp_dir().join(format!("w888-bit-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let (src, dst) = (d.join("a.bit"), d.join("xdevcfg"));
        std::fs::write(&src, vec![0xaa; 4096]).unwrap();
        std::fs::write(&dst, b"").unwrap();
        assert_eq!(load(&src, &dst).unwrap(), 4096);
        assert_eq!(std::fs::read(&dst).unwrap().len(), 4096);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
