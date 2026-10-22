// SPDX-License-Identifier: GPL-3.0-or-later
//! Bitstream loading. The ADC clock must already run (Si5351), and
//! `/dev/zynqsdr` is opened only after the load.
//!
//! * Vendor kernel: the legacy Xilinx devcfg device; the `.bit` file is
//!   copied in as is, as the stock system does.
//! * Current kernels (web888-ng-os): FPGA manager. The Zynq driver wants a
//!   `.bin` (no header, 32-bit words byte-swapped), loaded by name through
//!   the firmware loader, so it is converted here and written to
//!   [`FW_DIR`], which the image links from `/lib/firmware/web888` (the
//!   root file system is read-only).

use std::io;
use std::path::{Path, PathBuf};
use web888_radio::Variant;

pub const XDEVCFG: &str = "/dev/xdevcfg";
pub const FPGA_MGR: &str = "/sys/class/fpga_manager/fpga0";
/// Where the converted image goes; `/lib/firmware/web888` points here.
pub const FW_DIR: &str = "/run/web888/firmware";
/// Its name for the firmware loader (relative to `/lib/firmware`).
pub const FW_NAME: &str = "web888/web888-pl.bin";
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

/// The vendor kernel's devcfg driver is there. Checks for a character
/// device: without the driver, a stock userspace that writes its bitstream
/// to the path leaves a plain file behind.
pub fn has_devcfg() -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::metadata(XDEVCFG).is_ok_and(|m| m.file_type().is_char_device())
}

/// Copy `bit` into the devcfg device (`dev`, normally [`XDEVCFG`]).
pub fn load(bit: &Path, dev: &Path) -> io::Result<u64> {
    let mut src = std::fs::File::open(bit)?;
    let mut dst = std::fs::OpenOptions::new().write(true).open(dev)?;
    let n = io::copy(&mut src, &mut dst)?;
    dst.sync_all().ok();
    Ok(n)
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("bitstream: {what}"))
}

/// The configuration data of a `.bit` file (the `e` field); input without
/// a `.bit` header is returned unchanged.
pub fn bit_payload(b: &[u8]) -> io::Result<&[u8]> {
    const MAGIC: [u8; 13] = [
        0, 9, 0x0f, 0xf0, 0x0f, 0xf0, 0x0f, 0xf0, 0x0f, 0xf0, 0, 0, 1,
    ];
    if !b.starts_with(&MAGIC) {
        return Ok(b);
    }
    let mut p = MAGIC.len();
    loop {
        let key = *b.get(p).ok_or_else(|| bad("truncated header"))?;
        p += 1;
        if key == b'e' {
            let n = b.get(p..p + 4).ok_or_else(|| bad("truncated header"))?;
            let n = u32::from_be_bytes(n.try_into().unwrap()) as usize;
            return b.get(p + 4..p + 4 + n).ok_or_else(|| bad("truncated data"));
        }
        if !(b'a'..=b'd').contains(&key) {
            return Err(bad("unknown header field"));
        }
        let n = b.get(p..p + 2).ok_or_else(|| bad("truncated header"))?;
        p += 2 + u16::from_be_bytes(n.try_into().unwrap()) as usize;
    }
}

/// `.bit` or `.bin` (either byte order) to the Zynq FPGA manager's format:
/// 32-bit words byte-swapped, sync word as `66 55 99 aa`.
pub fn to_zynq_bin(file: &[u8]) -> io::Result<Vec<u8>> {
    let d = bit_payload(file)?;
    if d.len() % 4 != 0 {
        return Err(bad("length not a multiple of 4"));
    }
    let has = |w: [u8; 4]| d.as_chunks::<4>().0.contains(&w);
    if has([0x66, 0x55, 0x99, 0xaa]) {
        Ok(d.to_vec())
    } else if has([0xaa, 0x99, 0x55, 0x66]) {
        Ok(d.as_chunks::<4>()
            .0
            .iter()
            .flat_map(|&[a, b, c, d]| [d, c, b, a])
            .collect())
    } else {
        Err(bad("no sync word"))
    }
}

/// Load `bit` through the FPGA manager at `mgr` (normally [`FPGA_MGR`]):
/// convert it into `fw_dir` and request it as `fw_name`. Returns the
/// image size. A failed load is reported by the write to `firmware`.
pub fn load_fpga_mgr(bit: &Path, mgr: &Path, fw_dir: &Path, fw_name: &str) -> io::Result<u64> {
    let bin = to_zynq_bin(&std::fs::read(bit)?)?;
    std::fs::create_dir_all(fw_dir)?;
    let file = Path::new(fw_name)
        .file_name()
        .ok_or_else(|| bad("firmware name"))?;
    std::fs::write(fw_dir.join(file), &bin)?;
    std::fs::write(mgr.join("flags"), "0")?; // full reconfiguration
    std::fs::write(mgr.join("firmware"), fw_name)?;
    Ok(bin.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("w888-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A tiny `.bit`: header fields a..d, then padding, bus width, sync.
    fn bit_file(data: &[u8]) -> Vec<u8> {
        let mut b = vec![
            0, 9, 0x0f, 0xf0, 0x0f, 0xf0, 0x0f, 0xf0, 0x0f, 0xf0, 0, 0, 1,
        ];
        for (k, v) in [
            (b'a', "top;Version=1\0"),
            (b'b', "7z010clg400\0"),
            (b'c', "2026/10/22\0"),
            (b'd', "12:00:00\0"),
        ] {
            b.push(k);
            b.extend((v.len() as u16).to_be_bytes());
            b.extend(v.as_bytes());
        }
        b.push(b'e');
        b.extend((data.len() as u32).to_be_bytes());
        b.extend(data);
        b
    }

    const DATA: [u8; 16] = [
        0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0xbb, 0xaa, 0x99, 0x55, 0x66, 0x20, 0x00, 0x00,
        0x00,
    ];

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
        let d = tmp("bit");
        let (src, dst) = (d.join("a.bit"), d.join("xdevcfg"));
        std::fs::write(&src, vec![0xaa; 4096]).unwrap();
        std::fs::write(&dst, b"").unwrap();
        assert_eq!(load(&src, &dst).unwrap(), 4096);
        assert_eq!(std::fs::read(&dst).unwrap().len(), 4096);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn bit_to_swapped_bin() {
        let want = [
            0xff, 0xff, 0xff, 0xff, 0xbb, 0x00, 0x00, 0x00, 0x66, 0x55, 0x99, 0xaa, 0x00, 0x00,
            0x00, 0x20,
        ];
        assert_eq!(bit_payload(&bit_file(&DATA)).unwrap(), &DATA);
        assert_eq!(to_zynq_bin(&bit_file(&DATA)).unwrap(), want);
        // plain .bin in file order, and an already swapped one
        assert_eq!(to_zynq_bin(&DATA).unwrap(), want);
        assert_eq!(to_zynq_bin(&want).unwrap(), want);
        assert!(to_zynq_bin(&[0u8; 16]).is_err());
        let b = bit_file(&DATA);
        assert!(to_zynq_bin(&b[..b.len() - 4]).is_err());
    }

    #[test]
    fn loads_through_fpga_manager() {
        let d = tmp("mgr");
        let (mgr, fw) = (d.join("fpga0"), d.join("fw"));
        std::fs::create_dir_all(&mgr).unwrap();
        std::fs::write(d.join("a.bit"), bit_file(&DATA)).unwrap();
        assert_eq!(
            load_fpga_mgr(&d.join("a.bit"), &mgr, &fw, FW_NAME).unwrap(),
            16
        );
        assert_eq!(
            std::fs::read(fw.join("web888-pl.bin")).unwrap()[8..12],
            [0x66, 0x55, 0x99, 0xaa]
        );
        assert_eq!(std::fs::read_to_string(mgr.join("flags")).unwrap(), "0");
        assert_eq!(
            std::fs::read_to_string(mgr.join("firmware")).unwrap(),
            FW_NAME
        );
        std::fs::remove_dir_all(&d).unwrap();
    }
}
