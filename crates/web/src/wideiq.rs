// SPDX-License-Identifier: GPL-3.0-or-later
//! Wide IQ to the browser (`doc/m8-browser-decoders.md` item 9): one
//! channelizer bin (60 kS/s complex, clean within ±21 kHz of its centre)
//! straight from the bin tap, for the page's skimmers (a whole CW or RTTY
//! sub-band at once). The bin's samples are passed on as read: no DSP
//! here, the A9 reads the bin (~1 % of a core) and packs it.
//!
//! Frames (`KIND_IQ`): header (32 bytes) + `count` × (i8 I, i8 Q), or with
//! `FLAG_I16` (asked for with `bits: 16`, for recording) × (i16 I, i16 Q)
//! little-endian; each block scaled by its own peak:
//!
//! | offset | type | |
//! |---|---|---|
//! | 0 | u8 | kind = 3 |
//! | 1 | u8 | flags (`FLAG_GAP`: frames were lost before this one; `FLAG_I16`) |
//! | 2 | u16 | count (complex samples) |
//! | 4 | u32 | seq |
//! | 8 | u64 | the first sample's frame number |
//! | 16 | f64 | the bin's centre (Hz) |
//! | 24 | f32 | sample rate |
//! | 28 | f32 | scale: a sample is v × scale (1.0: a full-scale sine) |
//!
//! With `FLAG_TIME` (asked for with `timestamps: true`) an f64 follows:
//! the first sample's UTC (s) at the antenna, from the bins' timebase;
//! the samples then start at 40.

use crate::proto::{FLAG_GAP, FLAG_TIME};
use crate::workers::Out;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use web888_dsp::binrx::{BinTap, TapMsg};
use web888_radio::Iq32;
use web888_radio::bins::BinCaps;
use web888_radio::chan::RX_SINE_FULL_SCALE;

pub const KIND_IQ: u8 = 3;
pub const IQ_HEADER: usize = 32;
/// The samples are i16 (else i8).
pub const FLAG_I16: u8 = 4;
/// Samples a frame (34 ms at 60 kS/s).
pub const BLOCK: usize = 2048;
/// Wide streams at a time (each ~1 Mbit/s and a bin read).
pub const MAX_STREAMS: usize = 4;
/// The part of a bin that's clean (Hz from its centre).
pub const CLEAN_HZ: f64 = 21_000.0;

static ACTIVE: AtomicUsize = AtomicUsize::new(0);

/// The bin whose centre is nearest `hz`.
pub fn nearest_bin(caps: &BinCaps, hz: f64) -> usize {
    (0..caps.bins)
        .min_by(|&a, &b| {
            (caps.freq(a) - hz)
                .abs()
                .total_cmp(&(caps.freq(b) - hz).abs())
        })
        .unwrap_or(0)
}

/// A wide IQ frame (`i16`: 16-bit samples, `FLAG_I16` set; `utc`: the
/// first sample's time, `FLAG_TIME` set).
#[allow(clippy::too_many_arguments)]
pub fn iq_frame(
    flags: u8,
    seq: u32,
    frame: u64,
    centre_hz: f64,
    rate: f32,
    iq: &[Iq32],
    i16: bool,
    utc: Option<f64>,
) -> Vec<u8> {
    let peak = iq
        .iter()
        .map(|s| s.i.unsigned_abs().max(s.q.unsigned_abs()))
        .max()
        .unwrap_or(0)
        .max(1) as f64;
    let top = if i16 { 32767.0 } else { 127.0 };
    let step = peak / top;
    let mut b = Vec::with_capacity(IQ_HEADER + 8 + if i16 { 4 } else { 2 } * iq.len());
    let flags = if i16 { flags | FLAG_I16 } else { flags };
    b.push(KIND_IQ);
    b.push(if utc.is_some() {
        flags | FLAG_TIME
    } else {
        flags
    });
    b.extend_from_slice(&(iq.len() as u16).to_le_bytes());
    b.extend_from_slice(&seq.to_le_bytes());
    b.extend_from_slice(&frame.to_le_bytes());
    b.extend_from_slice(&centre_hz.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&((step / RX_SINE_FULL_SCALE) as f32).to_le_bytes());
    if let Some(t) = utc {
        b.extend_from_slice(&t.to_le_bytes());
    }
    for s in iq {
        let i = (s.i as f64 / step).round().clamp(-top, top);
        let q = (s.q as f64 / step).round().clamp(-top, top);
        if i16 {
            b.extend_from_slice(&(i as i16).to_le_bytes());
            b.extend_from_slice(&(q as i16).to_le_bytes());
        } else {
            b.push(i as i8 as u8);
            b.push(q as i8 as u8);
        }
    }
    b
}

/// A running stream; dropping it stops it.
pub struct Stream {
    stop: Arc<AtomicBool>,
    pub centre_hz: f64,
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Start streaming the bin nearest `hz` to `out` (`i16`: 16-bit samples;
/// `timed`: frames carry their time).
pub fn start(tap: &BinTap, hz: f64, out: Out, i16: bool, timed: bool) -> Result<Stream, String> {
    if ACTIVE.fetch_add(1, Ordering::SeqCst) >= MAX_STREAMS {
        ACTIVE.fetch_sub(1, Ordering::SeqCst);
        return Err(format!(
            "wide IQ: {MAX_STREAMS} streams already (try later)"
        ));
    }
    let caps = tap.caps();
    let bin = nearest_bin(&caps, hz);
    let centre = caps.freq(bin);
    let rate = caps.frame_rate as f32;
    let stop = Arc::new(AtomicBool::new(false));
    let (handle, rx) = tap.subscribe();
    handle.want(vec![bin], 0.0);
    let st = stop.clone();
    let tap = tap.clone();
    let spawned = std::thread::Builder::new()
        .name("wide-iq".into())
        .spawn(move || {
            let _handle = handle; // unsubscribes when the thread ends
            let mut buf: Vec<Iq32> = Vec::with_capacity(BLOCK);
            let mut buf_frame = 0u64;
            let mut next: Option<u64> = None;
            let mut gap = false;
            let mut seq = 0u32;
            while !st.load(Ordering::Relaxed) {
                let m = match rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(m) => m,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break,
                };
                let TapMsg::Frames(runs) = m else {
                    continue;
                };
                for f in runs.iter() {
                    let Some(idx) = f.bins.iter().position(|&b| b == bin) else {
                        gap = true; // our bin isn't in this run (the selection changing)
                        next = None;
                        continue;
                    };
                    if next.is_some_and(|n| n != f.first) {
                        gap = true;
                    }
                    for k in 0..f.count {
                        if buf.is_empty() {
                            buf_frame = f.first + k as u64;
                        }
                        buf.push(f.frame(k)[idx]);
                        if buf.len() == BLOCK {
                            let flags = if gap { FLAG_GAP } else { 0 };
                            gap = false;
                            let utc = if timed {
                                tap.timebase().utc(buf_frame as f64)
                            } else {
                                None
                            };
                            let f = iq_frame(flags, seq, buf_frame, centre, rate, &buf, i16, utc);
                            if !out.send(f) {
                                return ACTIVE.fetch_sub(1, Ordering::SeqCst);
                            }
                            seq = seq.wrapping_add(1);
                            buf.clear();
                        }
                    }
                    next = Some(f.end());
                }
            }
            ACTIVE.fetch_sub(1, Ordering::SeqCst)
        });
    if let Err(e) = spawned {
        ACTIVE.fetch_sub(1, Ordering::SeqCst);
        return Err(format!("wide IQ: {e}"));
    }
    Ok(Stream {
        stop,
        centre_hz: centre,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_layout() {
        let iq = [
            Iq32 {
                i: 1 << 29,
                q: -(1 << 28),
            },
            Iq32 { i: 0, q: 1 << 27 },
        ];
        let f = iq_frame(FLAG_GAP, 7, 1234, 7.02e6, 60_000.0, &iq, false, None);
        assert_eq!(f.len(), IQ_HEADER + 4);
        assert_eq!((f[0], f[1]), (KIND_IQ, FLAG_GAP));
        assert_eq!(u16::from_le_bytes([f[2], f[3]]), 2);
        assert_eq!(u32::from_le_bytes(f[4..8].try_into().unwrap()), 7);
        assert_eq!(u64::from_le_bytes(f[8..16].try_into().unwrap()), 1234);
        assert_eq!(f64::from_le_bytes(f[16..24].try_into().unwrap()), 7.02e6);
        assert_eq!(f32::from_le_bytes(f[24..28].try_into().unwrap()), 60_000.0);
        let scale = f32::from_le_bytes(f[28..32].try_into().unwrap());
        // the peak (a full-scale sine's I) is 127 steps
        assert_eq!(f[32] as i8, 127);
        assert!((127.0 * scale - 1.0).abs() < 1e-6);
        assert_eq!(f[33] as i8, -64);
        assert_eq!(f[35] as i8, 32);
    }

    #[test]
    fn frame_layout_16_bit() {
        let iq = [
            Iq32 {
                i: 1 << 29,
                q: -(1 << 28),
            },
            Iq32 { i: 0, q: 1 << 27 },
        ];
        let f = iq_frame(FLAG_GAP, 7, 1234, 7.02e6, 60_000.0, &iq, true, None);
        assert_eq!(f.len(), IQ_HEADER + 8);
        assert_eq!(f[1], FLAG_GAP | FLAG_I16);
        let scale = f32::from_le_bytes(f[28..32].try_into().unwrap());
        let v = |k: usize| i16::from_le_bytes([f[IQ_HEADER + 2 * k], f[IQ_HEADER + 2 * k + 1]]);
        assert_eq!((v(0), v(1), v(2), v(3)), (32767, -16384, 0, 8192));
        assert!((32767.0 * scale - 1.0).abs() < 1e-6);
    }

    #[test]
    fn bins_by_frequency() {
        let caps = BinCaps {
            bin_hz: 30_000.0,
            frame_rate: 60_000.0,
            bins: 2048,
            first_hz: 0.0,
            wraps: false,
            max_selected: 64,
        };
        assert_eq!(nearest_bin(&caps, 7_020_000.0), 234); // 7.02 MHz
        assert_eq!(nearest_bin(&caps, 7_004_000.0), 233); // 6.99 MHz is nearer than 7.02
    }
}
