// SPDX-License-Identifier: GPL-3.0-or-later
//! Wide audio from the channelizer's bins: a session's 24 kS/s audio
//! where its RX channel can't give it (the bins tuner's receivers are
//! 12 kS/s; the bins' 60 kS/s isn't a multiple of 24). The audio job
//! reads the bin its passband falls in from the bin tap (as wide IQ does:
//! about 1 % of an A9 core), turns the dial to 0 Hz, resamples it to
//! 48 kS/s, and the audio chain takes it from there as from an RX
//! channel. Its RX channel's blocks still pace the job.
//!
//! The bin is the one nearest the passband's centre; it is kept while the
//! passband stays within `wideiq::CLEAN_HZ` of its centre.

use crate::wideiq::{CLEAN_HZ, nearest_bin};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use web888_dsp::C32;
use web888_dsp::binrx::{BinTap, TapHandle, TapMsg};
use web888_dsp::resample::Resampler;

/// The rate the source hands the audio chain.
pub const RATE: f64 = 48_000.0;
/// Wide audio sessions at a time (each a bin read and a resampler).
pub const MAX_SOURCES: usize = 8;

static ACTIVE: AtomicUsize = AtomicUsize::new(0);

/// One session's bin, shifted and resampled.
pub struct BinSource {
    _handle: TapHandle,
    rx: Receiver<TapMsg>,
    bin: usize,
    centre_hz: f64,
    frame_rate: f64,
    rot: C32,
    step: C32,
    rs: Resampler,
    next: Option<u64>,
}

impl Drop for BinSource {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::SeqCst);
    }
}

/// L/M from `from` to `RATE` (both whole Hz).
fn ratio(from: f64) -> Option<(usize, usize)> {
    fn gcd(a: u64, b: u64) -> u64 {
        if b == 0 { a } else { gcd(b, a % b) }
    }
    let (a, b) = (RATE.round() as u64, from.round() as u64);
    if b == 0 {
        return None;
    }
    let g = gcd(a, b);
    let (l, m) = ((a / g) as usize, (b / g) as usize);
    (l <= 16 && m <= 16).then_some((l, m))
}

impl BinSource {
    /// The bin for a passband `lo..hi` Hz around `dial_hz`; `None` when
    /// all sources are taken or the bins' rate doesn't resample simply.
    pub fn new(tap: &BinTap, dial_hz: f64, lo: f64, hi: f64) -> Option<Self> {
        let caps = tap.caps();
        let (l, m) = ratio(caps.frame_rate)?;
        if ACTIVE.fetch_add(1, Ordering::SeqCst) >= MAX_SOURCES {
            ACTIVE.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        let bin = nearest_bin(&caps, dial_hz + 0.5 * (lo + hi));
        let (handle, rx) = tap.subscribe();
        handle.want(vec![bin], 0.0);
        let mut s = BinSource {
            _handle: handle,
            rx,
            bin,
            centre_hz: caps.freq(bin),
            frame_rate: caps.frame_rate,
            rot: C32::new(1.0, 0.0),
            step: C32::new(1.0, 0.0),
            rs: Resampler::new(l, m, 1),
            next: None,
        };
        s.retune(dial_hz);
        Some(s)
    }

    /// Whether this bin still carries the passband `lo..hi` around
    /// `dial_hz` cleanly.
    pub fn covers(&self, dial_hz: f64, lo: f64, hi: f64) -> bool {
        let off = dial_hz - self.centre_hz;
        (off + lo).abs() <= CLEAN_HZ && (off + hi).abs() <= CLEAN_HZ
    }

    /// Move the dial within the bin.
    pub fn retune(&mut self, dial_hz: f64) {
        let w = -2.0 * std::f64::consts::PI * (dial_hz - self.centre_hz) / self.frame_rate;
        self.step = C32::new(w.cos() as f32, w.sin() as f32);
    }

    /// The samples read since the last call, at `RATE`, appended to
    /// `out`; true if some were lost before them.
    pub fn read(&mut self, out: &mut Vec<C32>) -> bool {
        let mut gap = false;
        loop {
            let m = match self.rx.try_recv() {
                Ok(m) => m,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return gap,
            };
            let TapMsg::Frames(runs) = m else {
                continue;
            };
            for f in runs.iter() {
                let Ok(i) = f.bins.binary_search(&self.bin) else {
                    // our bin isn't in this run (the selection changing)
                    gap |= self.next.is_some();
                    self.next = None;
                    continue;
                };
                if self.next.is_some_and(|n| n != f.first) {
                    gap = true;
                }
                for k in 0..f.count {
                    let x = web888_dsp::rx_to_c32(f.frame(k)[i]) * self.rot;
                    self.rot *= self.step;
                    self.rs.push(&[x], out);
                }
                self.next = Some(f.end());
            }
        }
        // keep the rotator on the unit circle
        self.rot /= self.rot.norm();
        gap
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::AUDIO_HEADER;
    use crate::workers::{AudioCmd, AudioJob, Out, WfSink};
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;
    use web888_core::pool::Job;
    use web888_dsp::audio::{AudioSpec, Mode};
    use web888_dsp::chan::Droop;
    use web888_radio::bins::{BinCaps, BinFrames};
    use web888_radio::{Iq32, RX_SINE_FULL_SCALE, RxBlock};

    const CAPS: BinCaps = BinCaps {
        bin_hz: 30e3,
        frame_rate: 60e3,
        bins: 2048,
        first_hz: 0.0,
        wraps: false,
        max_selected: 64,
    };

    /// Frames `f0..f0 + n` of bin `k`: AM on `hz`, carrier 0.1 full scale,
    /// 50 % at `tone` Hz.
    fn am(k: usize, f0: u64, n: usize, hz: f64, tone: f64) -> Arc<Vec<BinFrames>> {
        let bins: Arc<[usize]> = Arc::from(vec![k]);
        let mut out = Vec::new();
        for frame in f0..f0 + n as u64 {
            let t = frame as f64 / CAPS.frame_rate;
            let env = 0.1 * (1.0 + 0.5 * (std::f64::consts::TAU * tone * t).cos());
            let p = std::f64::consts::TAU * (hz - CAPS.freq(k)) * t;
            let s = RX_SINE_FULL_SCALE * env;
            let iq = [Iq32 {
                i: (s * p.cos()) as i32,
                q: (s * p.sin()) as i32,
            }];
            BinFrames::append(&mut out, frame, 1, &bins, iq);
        }
        Arc::new(out)
    }

    /// Amplitude of `f` in `x` at `fs` (Hann).
    fn amp(x: &[f32], f: f64, fs: f64) -> f64 {
        let (mut re, mut im, mut ws) = (0.0, 0.0, 0.0);
        for (k, &v) in x.iter().enumerate() {
            let w = 0.5 - 0.5 * (std::f64::consts::TAU * k as f64 / (x.len() - 1) as f64).cos();
            let a = std::f64::consts::TAU * f * k as f64 / fs;
            re += v as f64 * w * a.cos();
            im -= v as f64 * w * a.sin();
            ws += w;
        }
        2.0 * (re * re + im * im).sqrt() / ws
    }

    #[test]
    fn a_12k_channel_gives_24k_audio_from_its_bin() {
        let tap = BinTap::new(CAPS);
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        let (tx, mut rx) = tokio::sync::mpsc::channel(10_000);
        let out = Out {
            tx,
            drops: Arc::new(AtomicU64::new(0)),
            total: None,
        };
        let (wtx, _wrx) = tokio::sync::mpsc::channel(10);
        let sink = WfSink::new(Out {
            tx: wtx,
            drops: Arc::new(AtomicU64::new(0)),
            total: None,
        });
        let spec = AudioSpec {
            rate: 24_000.0,
            lo_hz: -10_000.0,
            hi_hz: 10_000.0,
            ..AudioSpec::new(Mode::Am)
        };
        let mut job = AudioJob::new(Some(0), 12_000.0, Droop::None, spec, cmd_rx, out, sink)
            .with_tap(Some(tap.clone()));
        // 7.207 MHz: bin 240 (7.2 MHz), 7 kHz off its centre
        let dial = 7.207e6;
        cmd_tx.send(AudioCmd::Dial(dial)).unwrap();
        let block = |f0: u64| RxBlock {
            first_frame: f0,
            nch: 1,
            iq: vec![Iq32::default(); 600],
            overload: false,
        };
        assert!(job.rx(&block(0))); // subscribes
        tap.send_frames(am(240, 0, 120_000, dial, 8000.0));
        assert!(job.rx(&block(600)));
        let mut pcm = Vec::new();
        while let Ok(f) = rx.try_recv() {
            let rate = u32::from_le_bytes(f[8..12].try_into().unwrap());
            assert_eq!(rate, 24_000);
            pcm.extend(
                f[AUDIO_HEADER..]
                    .chunks(2)
                    .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32767.0),
            );
        }
        // 2 s of bin samples: 2 s of audio (less what the filters hold)
        assert!(pcm.len() > 47_000 && pcm.len() <= 48_000, "{}", pcm.len());
        // 50 % peaks at half the AM target (0.3)
        let a = amp(&pcm[12_000..], 8000.0, 24_000.0);
        assert!((a / 0.15 - 1.0).abs() < 0.05, "{a}");
        // back to 12 kS/s: the RX channel again
        cmd_tx
            .send(AudioCmd::Spec(AudioSpec {
                rate: 12_000.0,
                ..spec
            }))
            .unwrap();
        assert!(job.rx(&block(1200)));
        assert_eq!(ACTIVE.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn ratios() {
        assert_eq!(ratio(60_000.0), Some((4, 5)));
        assert_eq!(ratio(48_000.0), Some((1, 1)));
        assert_eq!(ratio(120_000.0), Some((2, 5)));
        assert_eq!(ratio(61_440.0), None);
    }
}
