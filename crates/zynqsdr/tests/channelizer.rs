// SPDX-License-Identifier: GPL-3.0-or-later
//! ZynqsdrRadio as a BinRadio against a fake channelizer driver: the mask
//! and gen it is given, frames and power lines in short reads that split
//! words and frames, the zoom engine's jobs and their results between the
//! power lines' chunks.

use std::collections::VecDeque;
use std::io;
use web888_dsp::binrx::{BinRx, TapMsg};
use web888_radio::bins::{BinFrames, BinRadio, ZoomCaps, ZoomJob, ZoomResult};
use web888_radio::*;
use web888_zynqsdr::abi::*;
use web888_zynqsdr::dev::Device;
use web888_zynqsdr::{
    ZynqConfig, ZynqsdrRadio, channelizer_clock, is_channelizer, tuner_r2, tuner_users,
};

const SIG: u32 = 0x0E5D_0010;
/// With the bins tuner: 32 users.
const SIG_TUNER: u32 = 0x0E5D_2010;
/// The VHF build (clock plan 1: 98.304 MHz, the tuner by 4).
const SIG_VHF: u32 = 0x0E5D_2110;

struct FakePfb {
    sig: u32,
    sets: Vec<(u32, u32)>,
    rx_params: Vec<(u32, u64)>,
    mask: [u32; 64],
    generation: u32,
    masks: Vec<(u32, Vec<usize>)>,
    starts: Vec<(u32, u32)>,
    streams: u32,
    frame: u32,
    line: u32,
    bins_q: Vec<u8>,
    power_q: Vec<u8>,
    /// The zoom engine's queue depth (0: none); `None`: a driver before
    /// the engine (its ioctls unknown).
    zoom: Option<usize>,
    /// Job words queued.
    jobs: VecDeque<u32>,
    /// `PFB_ZOOM` calls: (count, accepted).
    zoom_calls: Vec<(usize, usize)>,
}

impl FakePfb {
    fn new() -> Self {
        FakePfb {
            sig: SIG,
            sets: vec![],
            rx_params: vec![],
            mask: [0; 64],
            generation: 0,
            masks: vec![],
            starts: vec![],
            streams: 0,
            frame: 0,
            line: 0,
            bins_q: vec![],
            power_q: vec![],
            zoom: Some(4),
            jobs: VecDeque::new(),
            zoom_calls: vec![],
        }
    }
}

/// A zoom job's point i, as the engine writes it: (i + 1) times a
/// full-scale sine's (2^48).
fn zoom_point(i: usize) -> f32 {
    (i as f64 + 1.0) as f32 * 2f32.powi(48)
}

fn hdr(t: u64, sub: u64, num: u32) -> u64 {
    0x5042 << 48 | t << 44 | sub << 40 | num as u64
}

impl FakePfb {
    /// Three frames with the current mask: bin b reads {I: b, Q: -b}.
    fn produce(&mut self) {
        for _ in 0..3 {
            self.bins_q
                .extend(hdr(1, self.generation as u64, self.frame).to_le_bytes());
            for b in 0..2047usize {
                if self.mask[b / 32] >> (b % 32) & 1 == 1 {
                    let w = ((-(b as i32)) as u32 as u64) << 32 | b as u64;
                    self.bins_q.extend(w.to_le_bytes());
                }
            }
            self.frame += 1;
        }
        if self.streams & PFB_POWER != 0 {
            // a line: bin k's sum is k (full scale is 2^48 a frame); a
            // queued job's result after its chunk 3, another's after it
            for c in 0..16u64 {
                self.power_q.extend(hdr(2, c, self.line).to_le_bytes());
                for j in 0..64u64 {
                    let k = (c * 128 + 2 * j) as f32;
                    let w = ((k + 1.0).to_bits() as u64) << 32 | k.to_bits() as u64;
                    self.power_q.extend(w.to_le_bytes());
                }
                if c == 3 {
                    self.zoom_result();
                }
            }
            self.zoom_result();
            self.line += 1;
        }
    }

    /// The oldest queued job's result: `{tag, shift, logn, B - 1, k0}`;
    /// saturated if its shift is 0, its frame the current one.
    fn zoom_result(&mut self) {
        let Some(w) = self.jobs.pop_front() else {
            return;
        };
        let tag = (w >> 26) as u64 & 0x3f;
        let shift = w >> 23 & 7;
        let n = 1usize << (w >> 19 & 0xf);
        let bins = (w >> 11 & 0xff) as usize + 1;
        let points = bins * n / 2;
        let words = points.div_ceil(2);
        let chunks = words.div_ceil(64);
        let number =
            ((shift == 0) as u32) << 31 | ((chunks - 1) as u32) << 16 | self.frame & 0xffff;
        for c in 0..chunks {
            let h = hdr(3, c as u64, number) | tag << 32;
            self.power_q.extend(h.to_le_bytes());
            for j in 0..(words - 64 * c).min(64) {
                let i = 2 * (64 * c + j);
                let v = |i: usize| if i < points { zoom_point(i) } else { 0.0 };
                let w = (v(i + 1).to_bits() as u64) << 32 | v(i).to_bits() as u64;
                self.power_q.extend(w.to_le_bytes());
            }
        }
    }
}

impl Device for FakePfb {
    fn set(&mut self, cmd: u32, arg: u32) -> io::Result<()> {
        self.sets.push((cmd, arg));
        Ok(())
    }
    fn get_u32(&mut self, cmd: u32) -> io::Result<u32> {
        Ok(if cmd == GET_SIGNATURE { self.sig } else { 0 })
    }
    fn get_dna(&mut self) -> io::Result<u64> {
        Ok(1)
    }
    fn rx_param(&mut self, ch: u32, word: u64) -> io::Result<()> {
        if tuner_users(self.sig) == 0 {
            return Err(io::Error::from_raw_os_error(libc::EOPNOTSUPP));
        }
        self.rx_params.push((ch, word));
        Ok(())
    }
    fn wf_param(&mut self, _ch: u16, _d: u16, _word: u64) -> io::Result<()> {
        Err(io::Error::from_raw_os_error(libc::EOPNOTSUPP))
    }
    /// The tuner: three frames, user u reads {I: u, Q: -u}.
    fn rx_read(&mut self, dst: &mut [u8]) -> io::Result<usize> {
        if tuner_users(self.sig) == 0 {
            return Err(io::Error::from_raw_os_error(libc::EOPNOTSUPP));
        }
        let words: Vec<u8> = (0..3 * 32i32)
            .flat_map(|w| {
                let u: i32 = w % 32;
                [u.to_le_bytes(), (-u).to_le_bytes()].concat()
            })
            .collect();
        let n = words.len().min(dst.len());
        dst[..n].copy_from_slice(&words[..n]);
        Ok(n)
    }
    fn wf_read(&mut self, _ch: u16, _dst: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::from_raw_os_error(libc::EOPNOTSUPP))
    }
    fn pps_read(&mut self) -> io::Result<Option<u32>> {
        Ok(None)
    }
    fn pfb_set_mask(&mut self, generation: u32, mask: &[u32; 64]) -> io::Result<()> {
        self.mask = *mask;
        self.generation = generation;
        let bins = (0..2048)
            .filter(|b| mask[b / 32] >> (b % 32) & 1 == 1)
            .collect();
        self.masks.push((generation, bins));
        Ok(())
    }
    fn pfb_start(&mut self, streams: u32, power_frames: u32) -> io::Result<()> {
        self.starts.push((streams, power_frames));
        self.streams = streams;
        Ok(())
    }
    fn pfb_read(&mut self, stream: u32, dst: &mut [u8]) -> io::Result<usize> {
        // the streams not read lose their data, as a ring lapped
        let empty = if stream == PFB_BINS {
            self.bins_q.is_empty()
        } else {
            self.power_q.is_empty()
        };
        if empty {
            if self.streams & PFB_BINS == 0 {
                self.bins_q.clear();
            }
            if self.streams & PFB_POWER == 0 {
                self.power_q.clear();
            }
            self.produce();
        }
        let q = if stream == PFB_BINS {
            &mut self.bins_q
        } else {
            &mut self.power_q
        };
        // short, odd-sized reads
        let n = q.len().min(dst.len()).min(1001);
        dst[..n].copy_from_slice(&q[..n]);
        q.drain(..n);
        Ok(n)
    }
    fn pfb_zoom(&mut self, jobs: &[u32]) -> io::Result<usize> {
        let depth = match self.zoom {
            None => return Err(io::Error::from_raw_os_error(libc::ENOTTY)),
            Some(0) => return Err(io::Error::from_raw_os_error(libc::EOPNOTSUPP)),
            Some(d) => d,
        };
        if jobs.len() > PFB_ZOOM_MAX {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        if self.streams & PFB_POWER == 0 {
            return Err(io::Error::from_raw_os_error(libc::ENXIO));
        }
        let k = jobs.len().min(depth - self.jobs.len());
        self.jobs.extend(&jobs[..k]);
        self.zoom_calls.push((jobs.len(), k));
        Ok(k)
    }
    fn pfb_zoom_info(&mut self) -> io::Result<PfbZoomInfo> {
        let Some(depth) = self.zoom else {
            return Err(io::Error::from_raw_os_error(libc::ENOTTY));
        };
        Ok(PfbZoomInfo {
            size: 20,
            queue: depth as u32,
            free: (depth - self.jobs.len()) as u32,
            ..Default::default()
        })
    }
}

fn radio() -> ZynqsdrRadio<FakePfb> {
    ZynqsdrRadio::new(FakePfb::new(), ZynqConfig::default()).unwrap()
}

#[test]
fn channelizer_signature() {
    assert!(is_channelizer(SIG));
    assert!(!is_channelizer(0x0E5D_020C));
    assert!(!is_channelizer(0xAA55_0010));
    let mut r = radio();
    assert_eq!(r.caps().rx_channels, 0);
    assert!(matches!(r.set_rx_rate(5), Err(Error::Unsupported(_))));
    assert!(matches!(
        r.read_rx(&mut [Iq32::default(); 4]),
        Err(Error::Unsupported(_))
    ));
    let c = r.bin_caps();
    assert_eq!(
        (c.bins, c.max_selected, c.bin_hz, c.frame_rate),
        (2047, 64, 30e3, 60e3)
    );
    assert_eq!(c.bin_of(7.074e6), Some((236, -6000.0)));
}

#[test]
fn the_tuner_is_the_rx() {
    let mut dev = FakePfb::new();
    dev.sig = SIG_TUNER;
    assert_eq!(tuner_users(SIG_TUNER), 32);
    assert_eq!(tuner_users(SIG), 0);
    assert_eq!(tuner_users(0x0E5D_020C), 0);
    let mut r = ZynqsdrRadio::new(dev, ZynqConfig::default()).unwrap();
    let c = r.caps();
    assert_eq!((c.rx_channels, c.rx_r2.clone()), (32, 40..=40));
    // started at 12 kS/s once (a restart could split a frame); other rates
    // refused
    r.set_rx_rate(40).unwrap();
    let starts = r.device().sets.iter().filter(|s| s.0 == RX_START).count();
    assert_eq!(starts, 1);
    assert!(matches!(r.set_rx_rate(10), Err(Error::Unsupported(_))));
    r.tune_rx(5, 7.074e6).unwrap();
    let w = r.device().rx_params[0];
    assert_eq!(w.0, 5);
    assert!((w.1 as f64 / 2f64.powi(48) * 122.88e6 - 7.074e6).abs() < 1e-3);
    let mut buf = vec![Iq32::default(); 2 * 32];
    assert_eq!(r.read_rx(&mut buf).unwrap(), 2);
    assert_eq!(buf[33], Iq32 { i: 1, q: -1 });
    // the bins are still there for the waterfall
    assert_eq!(r.bin_caps().bins, 2047);
}

#[test]
fn the_vhf_build_undersamples_at_98_304_mhz() {
    assert_eq!(channelizer_clock(SIG_TUNER), Some(122.88e6));
    assert_eq!(channelizer_clock(SIG_VHF), Some(98.304e6));
    assert_eq!(channelizer_clock(0x0E5D_020D), None); // the tuned VHF build
    assert_eq!((tuner_r2(SIG_TUNER), tuner_r2(SIG_VHF)), (40, 32));
    let vhf = || {
        let mut d = FakePfb::new();
        d.sig = SIG_VHF;
        d
    };
    // each bitstream needs its clock
    let e = |r: Result<ZynqsdrRadio<FakePfb>>| match r {
        Err(Error::Unsupported(m)) => m,
        _ => panic!("expected an error"),
    };
    assert!(e(ZynqsdrRadio::new(vhf(), ZynqConfig::default())).contains("98.304"));
    let mut hf = FakePfb::new();
    hf.sig = SIG_TUNER;
    assert!(e(ZynqsdrRadio::new(hf, ZynqConfig::vhf())).contains("122.88"));

    let mut r = ZynqsdrRadio::new(vhf(), ZynqConfig::vhf()).unwrap();
    let c = r.caps();
    assert_eq!(c.variant, Variant::Vhf);
    assert_eq!((c.rx_channels, c.wf_channels), (32, 0));
    // the tuner at 12 kS/s: 98.304 MHz / (256 * 32)
    assert_eq!(c.rx_r2, 32..=32);
    assert_eq!(rx_rate(c.adc_hz, 32), 12e3);
    assert_eq!(c.rf_range, 98.304e6..=147.456e6);
    let starts: Vec<_> = r.device().sets.iter().filter(|s| s.0 == RX_START).collect();
    // RX_START takes the driver's token for the tuner (40) at any clock
    assert_eq!(starts, vec![&(RX_START, 40)]);
    // RF in, the baseband's word out: 125 MHz is 26.696 MHz upright
    r.tune_rx(3, 125.0e6).unwrap();
    let w = r.device().rx_params[0];
    assert_eq!(w.0, 3);
    assert!((nco_freq(w.1, 98.304e6) - 26.696e6).abs() < 1e-3);
    // the bins: 24 kHz apart at 48 kS/s, from the zone's edge
    let b = r.bin_caps();
    assert_eq!((b.bin_hz, b.frame_rate, b.first_hz), (24e3, 48e3, 98.304e6));
    let (k, off) = b.bin_of(125.0e6).unwrap();
    assert_eq!(k, 1112); // 26.688 MHz
    assert!((off - 8e3).abs() < 1e-6);
    assert!(b.bin_of(97e6).is_none());
    // the RF switch on the VHF path whatever the front end says
    r.front_end(FrontEnd::default()).unwrap();
    assert!(r.device().sets.contains(&(MODE_SET, 1)));
}

#[test]
fn odd_nyquist_zones_are_refused() {
    let mut d = FakePfb::new();
    d.sig = SIG_VHF;
    let cfg = ZynqConfig {
        nyquist_zone: 1,
        ..ZynqConfig::vhf()
    };
    assert!(matches!(
        ZynqsdrRadio::new(d, cfg),
        Err(Error::InvalidParam(_))
    ));
}

#[test]
fn bins_follow_the_selection_and_its_generation() {
    let mut r = radio();
    // unsorted, repeated, out of range: the source sorts and drops
    let g1 = r.set_bins(&[300, 5, 5, 2047, 4000]).unwrap();
    assert_eq!(g1, 1);
    assert_eq!(r.device().masks, vec![(1, vec![5, 300])]);
    assert_eq!(r.device().starts, vec![(PFB_BINS, 1)]);
    let mut out: Vec<BinFrames> = vec![];
    let mut n = 0;
    while n < 6 {
        n += r.read_bins(&mut out).unwrap();
    }
    let split = web888_radio::bins::split(&out);
    for f in &split {
        assert_eq!(f.generation, 1);
        assert_eq!(&*f.bins, &[5, 300]);
        assert_eq!(f.iq, vec![Iq32 { i: 5, q: -5 }, Iq32 { i: 300, q: -300 }]);
    }
    assert!(split.windows(2).all(|w| w[1].frame == w[0].frame + 1));

    // 70 bins: the lowest 64; generations cycle 1..=15
    let many: Vec<usize> = (100..170).collect();
    let g2 = r.set_bins(&many).unwrap();
    assert_eq!(g2, 2);
    assert_eq!(r.device().masks[1].1, (100..164).collect::<Vec<_>>());
    out.clear();
    let gen2 = |out: &[BinFrames]| {
        web888_radio::bins::split(out)
            .into_iter()
            .filter(|f| f.generation == 2)
            .collect::<Vec<_>>()
    };
    while gen2(&out).len() < 3 {
        r.read_bins(&mut out).unwrap();
    }
    let f = gen2(&out)[0].clone();
    assert_eq!(f.iq.len(), 64);
    assert_eq!(f.iq[63], Iq32 { i: 163, q: -163 });
    let gens: Vec<u8> = (0..15).map(|_| r.set_bins(&[1]).unwrap()).collect();
    assert_eq!(gens, [3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 1, 2]);
    // started once
    assert_eq!(r.device().starts.len(), 1);
}

#[test]
fn power_lines_scaled_and_stopped() {
    let mut r = radio();
    r.set_power(20).unwrap();
    assert_eq!(r.device().starts, vec![(PFB_POWER, 20)]);
    let mut line = vec![];
    let n = loop {
        if let Some(n) = r.read_power(&mut line).unwrap() {
            break n;
        }
    };
    assert_eq!(n, 0);
    assert_eq!(line.len(), 2048);
    let fs = (1u64 << 48) as f64 * 20.0;
    assert!((line[1000] as f64 * fs / 1000.0 - 1.0).abs() < 1e-6);
    // bins too, then the power line off: bins keep running
    r.set_bins(&[7]).unwrap();
    r.set_power(0).unwrap();
    assert_eq!(r.device().starts.last(), Some(&(PFB_BINS, 1)));
}

fn zjob(tag: u8, shift: u8, logn: u8, bins: u16, k0: u16) -> ZoomJob {
    ZoomJob {
        tag,
        shift,
        logn,
        bins,
        k0,
    }
}

/// Read until `n` results came (or give up).
fn results(r: &mut ZynqsdrRadio<FakePfb>, n: usize) -> Vec<ZoomResult> {
    let mut out = vec![];
    for _ in 0..1000 {
        if out.len() >= n {
            break;
        }
        r.read_zoom(&mut out).unwrap();
    }
    out
}

#[test]
fn the_zoom_engine_where_the_driver_has_one() {
    // the driver's queue (the fake's a short one), the bitstream's limits
    let short = ZoomCaps {
        queue: 4,
        ..ZoomCaps::ENGINE
    };
    assert_eq!(radio().zoom_caps(), Some(short));
    let mut dev = FakePfb::new();
    dev.zoom = Some(0); // a bitstream without the engine
    let mut r = ZynqsdrRadio::new(dev, ZynqConfig::default()).unwrap();
    assert_eq!(r.zoom_caps(), None);
    assert!(matches!(
        r.submit_zoom(&[zjob(1, 3, 7, 17, 228)]),
        Err(Error::Unsupported(_))
    ));
    assert_eq!(r.read_zoom(&mut vec![]).unwrap(), 0);
    // nothing started for it
    assert!(r.device().starts.is_empty());
}

#[test]
fn zoom_jobs_queued_as_far_as_there_is_room() {
    let mut r = radio();
    let jobs: Vec<ZoomJob> = (0..6).map(|t| zjob(t, 3, 3, 1, 100 + t as u16)).collect();
    // the power stream started for the results, at the slowest line rate
    assert_eq!(r.submit_zoom(&jobs).unwrap(), 4);
    assert_eq!(r.device().starts, vec![(PFB_POWER, 65535)]);
    assert_eq!(r.submit_zoom(&jobs[4..]).unwrap(), 0);
    let words: Vec<u32> = r.device().jobs.iter().copied().collect();
    assert_eq!(
        words,
        jobs[..4].iter().map(|j| j.word()).collect::<Vec<_>>()
    );
    // more than a call carries: in calls of 16, stopping at the first short
    let many = vec![zjob(9, 3, 3, 1, 5); 40];
    r.device().zoom = Some(64);
    r.device().jobs.clear();
    r.device().zoom_calls.clear();
    assert_eq!(r.submit_zoom(&many).unwrap(), 40);
    assert_eq!(r.device().zoom_calls, vec![(16, 16), (16, 16), (8, 8)]);
    r.device().jobs.clear();
    r.device().zoom_calls.clear();
    r.device().zoom = Some(20);
    assert_eq!(r.submit_zoom(&many).unwrap(), 20);
    assert_eq!(r.device().zoom_calls, vec![(16, 16), (16, 4)]);
}

#[test]
fn zoom_results_assembled_and_scaled() {
    let mut r = radio();
    // 17 bins of 128 (1088 points, 9 chunks), 1 of 8 (4 points), saturated
    let jobs = [zjob(5, 3, 7, 17, 228), zjob(6, 0, 3, 1, 900)];
    assert_eq!(r.submit_zoom(&jobs).unwrap(), 2);
    let got = results(&mut r, 2);
    assert_eq!(got.len(), 2);
    assert_eq!(
        (got[0].tag, got[0].saturated, got[0].power.len()),
        (5, false, 1088)
    );
    assert_eq!(
        (got[1].tag, got[1].saturated, got[1].power.len()),
        (6, true, 4)
    );
    // the trait's scale: a full-scale sine's 2^48 reads 1
    for r in &got {
        for (i, &x) in r.power.iter().enumerate() {
            assert!(
                (x / (i as f32 + 1.0) - 1.0).abs() < 1e-6,
                "{} {i} {x}",
                r.tag
            );
        }
    }
    // no lines wanted: none handed out
    assert_eq!(r.read_power(&mut vec![]).unwrap(), None);
    // each result taken once
    assert!(results(&mut r, 1).is_empty());
}

#[test]
fn power_lines_and_zoom_results_together() {
    let mut r = radio();
    r.set_power(20).unwrap();
    assert_eq!(r.submit_zoom(&[zjob(1, 3, 7, 17, 228)]).unwrap(), 1);
    // already running: not restarted
    assert_eq!(r.device().starts, vec![(PFB_POWER, 20)]);
    let mut line = vec![];
    let mut zoom = vec![];
    let mut lines = 0;
    for _ in 0..100 {
        while r.read_power(&mut line).unwrap().is_some() {
            assert!((line[1000] as f64 * 20.0 * 2f64.powi(48) / 1000.0 - 1.0).abs() < 1e-6);
            lines += 1;
        }
        r.read_zoom(&mut zoom).unwrap();
        if lines >= 3 && !zoom.is_empty() {
            break;
        }
    }
    assert!(lines >= 3);
    assert_eq!(zoom.len(), 1);
    // no lines wanted any more: the stream stays for the engine
    r.set_power(0).unwrap();
    assert_eq!(r.device().starts.last(), Some(&(PFB_POWER, 65535)));
    assert_eq!(r.read_power(&mut line).unwrap(), None);
}

#[test]
fn set_power_keeps_the_jobs_in_flight() {
    let mut r = radio();
    r.set_power(20).unwrap();
    let jobs = [zjob(1, 3, 7, 17, 228), zjob(2, 3, 3, 1, 5)];
    assert_eq!(r.submit_zoom(&jobs).unwrap(), 2);
    // another line rate before the results came
    r.set_power(60).unwrap();
    let got = results(&mut r, 2);
    let tags: Vec<(u8, usize)> = got.iter().map(|z| (z.tag, z.power.len())).collect();
    assert_eq!(tags, vec![(1, 1088), (2, 4)]);
    // the lines at the new scale
    let mut line = vec![];
    let fs = 60.0 * 2f64.powi(48);
    let mut n = 0;
    while n < 2 {
        if r.read_power(&mut line).unwrap().is_some() {
            assert!((line[1000] as f64 * fs / 1000.0 - 1.0).abs() < 1e-6);
            n += 1;
        }
    }
}

#[test]
fn an_old_driver_has_no_engine_and_views_use_bursts() {
    let mut dev = FakePfb::new();
    dev.zoom = None;
    let r = ZynqsdrRadio::new(dev, ZynqConfig::default()).unwrap();
    assert_eq!(r.zoom_caps(), None);
    let mut rx = BinRx::tap_only(r).unwrap();
    let tap = rx.tap();
    // no engine on the tap: views read bursts of frames and run the FFTs
    assert_eq!(tap.zoom_caps(), None);
    let (h, _rx) = tap.subscribe();
    assert!(h.submit_zoom(&[zjob(0, 3, 7, 17, 228)], false).is_empty());
    assert!(rx.inner().device().zoom_calls.is_empty());
}

#[test]
fn the_server_runs_the_power_stream_for_the_engine() {
    let mut dev = FakePfb::new();
    dev.sig = SIG_TUNER;
    let r = ZynqsdrRadio::new(dev, ZynqConfig::default()).unwrap();
    let mut rx = BinRx::tap_only(r).unwrap();
    let tap = rx.tap();
    assert_eq!(tap.zoom_caps().map(|z| z.queue), Some(4));
    // a view wanting no power line, only jobs
    let (h, hrx) = tap.subscribe();
    let tags = h.submit_zoom(&[zjob(0, 3, 7, 17, 228), zjob(0, 3, 3, 2, 50)], false);
    assert_eq!(tags, vec![0, 1]);
    let mut buf = vec![Iq32::default(); 32 * 4];
    let mut got = vec![];
    for _ in 0..200 {
        rx.read_rx(&mut buf).unwrap();
        while let Ok(m) = hrx.try_recv() {
            match m {
                TapMsg::Zoom(z) => got.push((z.tag, z.power.len(), z.power[0])),
                TapMsg::Power(_) => panic!("no lines wanted"),
                TapMsg::Frames(_) => {}
            }
        }
        if got.len() == 2 {
            break;
        }
    }
    assert_eq!(got, vec![(0, 1088, 1.0), (1, 8, 1.0)]);
    let starts = &rx.inner().device().starts;
    assert!(
        starts.contains(&(PFB_BINS | PFB_POWER, 65535)),
        "{starts:?}"
    );
}
