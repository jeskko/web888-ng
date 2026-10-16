// SPDX-License-Identifier: GPL-3.0-or-later
//! ZynqsdrRadio against a fake driver that behaves like the stock one
//! (short reads that split frames, scalar/struct ioctls), with sample data
//! from the simulator.

use std::io;
use std::time::Duration;
use web888_radio::*;
use web888_sim::*;
use web888_zynqsdr::abi::*;
use web888_zynqsdr::dev::Device;
use web888_zynqsdr::{ZynqConfig, ZynqsdrRadio};

const F0: f64 = 7_074_000.0;

struct FakeDriver {
    sim: SimRadio,
    log: Vec<(u32, u32)>,
    gpio: u32,
    /// Max bytes per RX_READ (models short reads).
    chunk: usize,
    rx_q: Vec<u8>,
    wf_param: [(u16, u64); 2],
    /// Emulate a bitstream whose WF stops after one block (one-shot).
    wf_oneshot_hw: bool,
}

impl FakeDriver {
    fn new(scene: Scene) -> Self {
        FakeDriver {
            sim: SimRadio::new(SimConfig::hf(scene, Pacing::Free)),
            log: Vec::new(),
            gpio: 0x200, // a bit we don't own, must be preserved
            chunk: 1000,
            rx_q: Vec::new(),
            wf_param: [(1, 0); 2],
            wf_oneshot_hw: false,
        }
    }
}

impl Device for FakeDriver {
    fn set(&mut self, cmd: u32, arg: u32) -> io::Result<()> {
        self.log.push((cmd, arg));
        match cmd {
            RX_START => {
                self.sim.set_rx_rate(arg as u16).map_err(io::Error::other)?;
                self.rx_q.clear();
            }
            SET_GPIO_MASK => self.gpio = arg,
            WF_START => {
                let slot = (arg & 0xff) as usize;
                let (decim, word) = self.wf_param[slot];
                let f = nco_freq(word, self.sim.adc_hz());
                let mode = if self.wf_oneshot_hw || arg & WF_READ_CONTINUES != 0 {
                    WfMode::OneShot
                } else {
                    WfMode::Continuous // open bitstream: mode bit set = continuous
                };
                self.sim
                    .start_wf(slot, f, decim, mode)
                    .map_err(io::Error::other)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn get_u32(&mut self, cmd: u32) -> io::Result<u32> {
        match cmd {
            GET_SIGNATURE => Ok(0xAA55_020C),
            GET_GPIO_MASK => Ok(self.gpio),
            _ => Err(io::Error::from_raw_os_error(libc_einval())),
        }
    }

    fn get_dna(&mut self) -> io::Result<u64> {
        Ok(0x0012_3456_789a_bcde)
    }

    fn rx_param(&mut self, ch: u32, word: u64) -> io::Result<()> {
        let f = nco_freq(word, self.sim.adc_hz());
        self.sim.tune_rx(ch as usize, f).map_err(io::Error::other)
    }

    fn wf_param(&mut self, ch: u16, decim: u16, word: u64) -> io::Result<()> {
        self.wf_param[ch as usize] = (decim, word);
        Ok(())
    }

    fn rx_read(&mut self, dst: &mut [u8]) -> io::Result<usize> {
        let n = self.sim.caps().rx_channels;
        while self.rx_q.len() < dst.len().min(self.chunk) {
            let mut fr = vec![Iq32::default(); 64 * n];
            let got = self.sim.read_rx(&mut fr).map_err(io::Error::other)?;
            for s in &fr[..got * n] {
                self.rx_q.extend_from_slice(&s.i.to_le_bytes());
                self.rx_q.extend_from_slice(&s.q.to_le_bytes());
            }
        }
        let k = dst.len().min(self.chunk).min(self.rx_q.len());
        dst[..k].copy_from_slice(&self.rx_q[..k]);
        self.rx_q.drain(..k);
        Ok(k)
    }

    fn wf_read(&mut self, ch: u16, dst: &mut [u8]) -> io::Result<usize> {
        let mut s = vec![Iq16::default(); (dst.len() / 4).min(777)];
        let got = self
            .sim
            .read_wf(ch as usize, &mut s)
            .map_err(io::Error::other)?;
        for (k, v) in s[..got].iter().enumerate() {
            dst[4 * k..4 * k + 2].copy_from_slice(&v.i.to_le_bytes());
            dst[4 * k + 2..4 * k + 4].copy_from_slice(&v.q.to_le_bytes());
        }
        Ok(got * 4)
    }

    fn pps_read(&mut self) -> io::Result<Option<u32>> {
        self.sim.pps().map_err(io::Error::other)
    }
}

fn libc_einval() -> i32 {
    22
}

fn radio(scene: Scene) -> ZynqsdrRadio<FakeDriver> {
    ZynqsdrRadio::new(FakeDriver::new(scene), ZynqConfig::default()).unwrap()
}

fn mean_freq(z: &[(f64, f64)], fs: f64) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for w in z.windows(2) {
        // w1 * conj(w0)
        re += w[1].0 * w[0].0 + w[1].1 * w[0].1;
        im += w[1].1 * w[0].0 - w[1].0 * w[0].1;
    }
    im.atan2(re) * fs / (2.0 * std::f64::consts::PI)
}

#[test]
fn init_sequence_and_identity() {
    let mut r = radio(Scene::new());
    assert_eq!(r.caps().rx_channels, 12);
    assert_eq!(r.caps().wf_channels, 2);
    let id = r.identity();
    assert_eq!(id.signature, 0xAA55_020C);
    assert_eq!(id.dna, Some(0x0012_3456_789a_bcde));
    let log = &r.device().log;
    assert_eq!(log[0], (PPS_START, 1));
    assert_eq!(log[1], (RX_START, 20));
}

#[test]
fn rx_frames_survive_short_reads() {
    let mut r = radio(Scene::new().tone(F0 + 1000.0, -20.0));
    r.tune_rx(3, F0).unwrap();
    r.tune_rx(7, F0 + 3000.0).unwrap();
    let n = 12;
    let mut z3 = Vec::new();
    let mut z7 = Vec::new();
    let mut buf = vec![Iq32::default(); 50 * n];
    while z3.len() < 4000 {
        let got = r.read_rx(&mut buf).unwrap();
        for f in 0..got {
            let a = buf[f * n + 3];
            let b = buf[f * n + 7];
            z3.push((a.i as f64, a.q as f64));
            z7.push((b.i as f64, b.q as f64));
        }
    }
    let fs = rx_rate(ADC_HZ_HF, 20);
    assert!((mean_freq(&z3, fs) - 1000.0).abs() < 0.01);
    assert!(
        (mean_freq(&z7, fs) + 2000.0).abs() < 0.01,
        "channel order kept"
    );
    let mag = z3.iter().map(|(i, q)| (i * i + q * q).sqrt()).sum::<f64>() / z3.len() as f64;
    let want = RX_SINE_FULL_SCALE * 0.1 * rx_chain_mag(1000.0, ADC_HZ_HF, 20);
    assert!((mag / want - 1.0).abs() < 1e-6);
    assert!(matches!(
        r.read_rx(&mut [Iq32::default(); 13]),
        Err(Error::BadBuffer)
    ));
    assert!(matches!(r.tune_rx(12, F0), Err(Error::InvalidChannel(12))));
}

#[test]
fn front_end_ioctls() {
    let mut r = radio(Scene::new());
    r.front_end(FrontEnd {
        atten_db: 10.5,
        antenna: 0b100101,
        dither: true,
        pga: false,
        led: true,
        vhf_path: true,
        ext_ref: false,
    })
    .unwrap();
    let log = &r.device().log;
    assert!(log.contains(&(AD8370_SET, 21)));
    assert!(log.contains(&(MODE_SET, 1)));
    assert!(log.contains(&(CLK_SET, 1)));
    let g = 0x200 | 0b100101 | GPIO_DITHER | GPIO_LED;
    assert_eq!(*log.last().unwrap(), (SET_GPIO_MASK, g));
    assert!(
        r.front_end(FrontEnd {
            atten_db: 40.0,
            ..Default::default()
        })
        .is_err()
    );
}

#[test]
fn wf_one_shot_and_never_sets_continue_flag() {
    let mut r = radio(Scene::new().tone(F0 + 250_000.0, -6.0));
    r.start_wf(0, F0, 16, WfMode::OneShot).unwrap();
    let mut buf = vec![Iq16::default(); 1000];
    let mut total = 0;
    loop {
        let n = r.read_wf(0, &mut buf).unwrap();
        if n == 0 {
            break;
        }
        total += n;
    }
    assert_eq!(total, WF_BLOCK);
    assert!(
        r.device()
            .log
            .iter()
            .all(|&(c, a)| c != WF_START || a & WF_READ_CONTINUES == 0)
    );
}

#[test]
fn wf_continuous_restarts_on_one_shot_hardware() {
    let mut drv = FakeDriver::new(Scene::new().tone(F0 + 1e6, -6.0));
    drv.wf_oneshot_hw = true;
    let mut r = ZynqsdrRadio::new(drv, ZynqConfig::default()).unwrap();
    r.start_wf(1, F0, 1, WfMode::Continuous).unwrap();
    let mut buf = vec![Iq16::default(); 4096];
    let mut total = 0;
    for _ in 0..3 {
        r.read_wf(1, &mut buf).unwrap(); // drains the block, then stalls
    }
    while total < 3 * WF_BLOCK {
        let n = r.read_wf(1, &mut buf).unwrap();
        total += n;
        if n == 0 {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    let starts = r
        .device()
        .log
        .iter()
        .filter(|&&(c, _)| c == WF_START)
        .count();
    assert!(starts >= 3, "restarted {starts} times");
}
