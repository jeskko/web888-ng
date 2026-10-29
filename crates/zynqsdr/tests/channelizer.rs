// SPDX-License-Identifier: GPL-3.0-or-later
//! ZynqsdrRadio as a BinRadio against a fake channelizer driver: the mask
//! and gen it is given, frames and power lines in short reads that split
//! words and frames.

use std::io;
use web888_radio::bins::{BinFrames, BinRadio};
use web888_radio::*;
use web888_zynqsdr::abi::*;
use web888_zynqsdr::dev::Device;
use web888_zynqsdr::{ZynqConfig, ZynqsdrRadio, is_channelizer, tuner_users};

const SIG: u32 = 0x0E5D_0010;
/// With the bins tuner: 32 users.
const SIG_TUNER: u32 = 0x0E5D_2010;

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
        }
    }
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
            // a line: bin k's sum is k (full scale is 2^48 a frame)
            for c in 0..16u64 {
                self.power_q.extend(hdr(2, c, self.line).to_le_bytes());
                for j in 0..64u64 {
                    let k = (c * 128 + 2 * j) as f32;
                    let w = ((k + 1.0).to_bits() as u64) << 32 | k.to_bits() as u64;
                    self.power_q.extend(w.to_le_bytes());
                }
            }
            self.line += 1;
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
        if self.sig != SIG_TUNER {
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
        if self.sig != SIG_TUNER {
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
        if self.bins_q.is_empty() && self.power_q.is_empty() {
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
