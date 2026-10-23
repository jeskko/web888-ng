// SPDX-License-Identifier: GPL-3.0-or-later
//! Minimal reader for mono 16-bit PCM WAV files (test corpora, tools).

use std::io::{self, Read};

/// Samples scaled to ±1, and the sample rate.
pub fn read(path: &std::path::Path) -> io::Result<(Vec<f32>, u32)> {
    let mut b = vec![];
    std::fs::File::open(path)?.read_to_end(&mut b)?;
    let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidData, m.to_string());
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err(bad("not a WAV file"));
    }
    let mut pos = 12;
    let mut rate = 0;
    let mut fmt_ok = false;
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let len = u32::from_le_bytes(b[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = &b[pos + 8..(pos + 8 + len).min(b.len())];
        if id == b"fmt " && body.len() >= 16 {
            let format = u16::from_le_bytes([body[0], body[1]]);
            let channels = u16::from_le_bytes([body[2], body[3]]);
            rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
            let bits = u16::from_le_bytes([body[14], body[15]]);
            fmt_ok = format == 1 && channels == 1 && bits == 16;
        } else if id == b"data" {
            if !fmt_ok {
                return Err(bad("only mono 16-bit PCM"));
            }
            let s = body
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
                .collect();
            return Ok((s, rate));
        }
        pos += 8 + len + (len & 1);
    }
    Err(bad("no data chunk"))
}

/// Write mono 16-bit PCM; samples are ±1 full scale.
pub fn write(path: &std::path::Path, x: &[f32], rate: u32) -> io::Result<()> {
    let mut b = Vec::with_capacity(44 + 2 * x.len());
    let data = (2 * x.len()) as u32;
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(2 * rate).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for &v in x {
        b.extend_from_slice(&((v.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    std::fs::write(path, b)
}
