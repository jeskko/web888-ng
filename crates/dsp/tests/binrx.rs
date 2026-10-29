// SPDX-License-Identifier: GPL-3.0-or-later
//! BinRx: RX channels from a fake bin source with a tone in one bin.

use std::f64::consts::PI;
use std::sync::Arc;
use web888_dsp::binrx::{BinRx, TapMsg};
use web888_radio::bins::{BinCaps, BinFrames, BinRadio, selection};
use web888_radio::*;

const TONE_BIN: usize = 236; // 7.080 MHz
const TONE_OFF: f64 = -6000.0; // the tone at 7.074 MHz
const AMP: f64 = 1e8;

/// Bins 30 kHz apart at 60 kS/s, like the channelizer bitstream; bin
/// TONE_BIN carries a tone TONE_OFF from its centre, the others nothing.
struct FakeBins {
    sel: Arc<[usize]>,
    generation: u8,
    frame: u64,
    selections: Vec<Vec<usize>>,
    power_frames: u16,
    lines: u32,
}

impl FakeBins {
    fn new() -> Self {
        FakeBins {
            sel: Arc::from(Vec::new()),
            generation: 0,
            frame: 0,
            selections: vec![],
            power_frames: 0,
            lines: 0,
        }
    }
}

impl BinRadio for FakeBins {
    fn bin_caps(&self) -> BinCaps {
        BinCaps {
            bin_hz: 30e3,
            frame_rate: 60e3,
            bins: 2047,
            first_hz: 0.0,
            wraps: false,
            max_selected: 64,
        }
    }
    fn set_bins(&mut self, bins: &[usize]) -> Result<u8> {
        let s = selection(bins, 2047, 64);
        self.selections.push(s.clone());
        self.sel = Arc::from(s);
        self.generation = self.generation % 15 + 1;
        Ok(self.generation)
    }
    fn read_bins(&mut self, out: &mut Vec<BinFrames>) -> Result<usize> {
        for _ in 0..300 {
            let t = self.frame as f64 / 60e3;
            let iq = self.sel.iter().map(|&b| {
                if b == TONE_BIN {
                    let p = 2.0 * PI * TONE_OFF * t;
                    Iq32 {
                        i: (AMP * p.cos()) as i32,
                        q: (AMP * p.sin()) as i32,
                    }
                } else {
                    Iq32::default()
                }
            });
            BinFrames::append(out, self.frame, self.generation, &self.sel, iq);
            self.frame += 1;
        }
        Ok(300)
    }
    fn set_power(&mut self, frames: u16) -> Result<()> {
        self.power_frames = frames;
        Ok(())
    }
    /// A line per batch while lines are on (the tone's bin at 1).
    fn read_power(&mut self, line: &mut Vec<f32>) -> Result<Option<u32>> {
        if self.power_frames == 0 || self.lines as u64 * 300 >= self.frame {
            return Ok(None);
        }
        *line = vec![0.0; 2047];
        line[TONE_BIN] = 1.0;
        self.lines += 1;
        Ok(Some(self.lines))
    }
}

impl Radio for FakeBins {
    fn caps(&self) -> Caps {
        Caps {
            variant: Variant::Hf,
            rx_channels: 0,
            wf_channels: 0,
            adc_hz: ADC_HZ_HF,
            rx_r2: 5..=40,
            wf_decim_min: 1,
            wf_decim_max: 8192,
            wf_block: 8192,
            sample_counter: false,
            rf_range: 0.0..=61.44e6,
        }
    }
    fn adc_hz(&self) -> f64 {
        ADC_HZ_HF
    }
    fn set_adc_hz(&mut self, _hz: f64) {}
    fn set_rx_rate(&mut self, _r2: u16) -> Result<()> {
        Err(Error::Unsupported("no RX"))
    }
    fn rx_r2(&self) -> u16 {
        0
    }
    fn tune_rx(&mut self, ch: usize, _f: f64) -> Result<()> {
        Err(Error::InvalidChannel(ch))
    }
    fn read_rx(&mut self, _buf: &mut [Iq32]) -> Result<usize> {
        Err(Error::Unsupported("no RX"))
    }
    fn start_wf(&mut self, slot: usize, _f: f64, _d: u16, _m: WfMode) -> Result<()> {
        Err(Error::InvalidChannel(slot))
    }
    fn read_wf(&mut self, slot: usize, _buf: &mut [Iq16]) -> Result<usize> {
        Err(Error::InvalidChannel(slot))
    }
    fn front_end(&mut self, _fe: FrontEnd) -> Result<()> {
        Ok(())
    }
    fn pps(&mut self) -> Result<Option<u32>> {
        Ok(None)
    }
    fn sample_counter(&mut self) -> Option<u64> {
        None
    }
    fn identity(&self) -> Identity {
        Identity {
            backend: "fake",
            signature: 0,
            dna: None,
            build_id: None,
            live: false,
        }
    }
}

/// Mean frequency (Hz) and RMS of a complex stream at `fs`.
fn tone(z: &[(f64, f64)], fs: f64) -> (f64, f64) {
    let mut ph = 0.0;
    for w in z.windows(2) {
        let (a, b) = (w[0], w[1]);
        // arg(b · conj(a))
        ph += (b.1 * a.0 - b.0 * a.1).atan2(b.0 * a.0 + b.1 * a.1);
    }
    let f = ph / (z.len() - 1) as f64 * fs / (2.0 * PI);
    let rms = (z.iter().map(|(i, q)| i * i + q * q).sum::<f64>() / z.len() as f64).sqrt();
    (f, rms)
}

#[test]
fn channels_from_bins() {
    let mut r = BinRx::new(FakeBins::new(), 4).unwrap();
    assert_eq!(r.caps().rx_channels, 4);
    assert_eq!(r.rx_r2(), 10);
    r.tune_rx(0, 7.074e6).unwrap(); // on the tone
    r.tune_rx(1, 7.075e6).unwrap(); // 1 kHz above it
    r.tune_rx(3, 14.074e6).unwrap(); // another bin, empty
    assert_eq!(r.inner().selections.last().unwrap(), &vec![236, 469]);
    assert!(r.tune_rx(4, 7e6).is_err());
    assert!(r.tune_rx(0, 70e6).is_err());

    // 3000 bin frames -> 2400 RX frames at 48 kS/s
    let mut buf = vec![Iq32::default(); 4 * 512];
    let mut z: Vec<Vec<(f64, f64)>> = vec![vec![]; 4];
    let mut frames = 0;
    while frames < 4800 {
        let n = r.read_rx(&mut buf).unwrap();
        for k in 0..n {
            for c in 0..4 {
                let s = buf[k * 4 + c];
                z[c].push((s.i as f64, s.q as f64));
            }
        }
        frames += n;
    }
    let skip = 200; // the resampler filling
    let (f0, a0) = tone(&z[0][skip..], 48e3);
    let (f1, a1) = tone(&z[1][skip..], 48e3);
    let (_, a3) = tone(&z[3][skip..], 48e3);
    let (_, a2) = tone(&z[2][skip..], 48e3);
    assert!(f0.abs() < 1.0, "channel 0 at {f0} Hz");
    assert!((f1 + 1000.0).abs() < 1.0, "channel 1 at {f1} Hz");
    assert!((a0 / AMP - 1.0).abs() < 0.01, "level {}", a0 / AMP);
    assert!((a1 / AMP - 1.0).abs() < 0.01, "level {}", a1 / AMP);
    assert_eq!((a2, a3), (0.0, 0.0));

    // idle: its bin leaves the selection once nobody uses it
    r.idle_rx(3).unwrap();
    assert_eq!(r.inner().selections.last().unwrap(), &vec![236]);
    r.idle_rx(1).unwrap();
    assert_eq!(r.inner().selections.last().unwrap(), &vec![236]);
}

#[test]
fn rates_follow_r2() {
    let mut r = BinRx::new(FakeBins::new(), 2).unwrap();
    r.tune_rx(0, 7.074e6).unwrap();
    for (r2, num, den) in [(10u16, 4, 5), (8, 1, 1), (20, 2, 5), (40, 1, 5), (5, 8, 5)] {
        r.set_rx_rate(r2).unwrap();
        let f0 = r.inner().frame;
        let mut buf = vec![Iq32::default(); 2 * 64];
        let mut got = 0u64;
        while r.inner().frame - f0 < 6000 {
            got += r.read_rx(&mut buf).unwrap() as u64;
        }
        let fed = r.inner().frame - f0;
        // what was fed, at num/den, less what waits in the output buffer
        // (at most one batch of the fake: 300 bin frames)
        let want = fed * num / den;
        assert!(
            got <= want && want - got <= 300 * num / den,
            "r2 {r2}: {got} of {want}"
        );
        // and the tone still at 0 Hz at the new rate
        let fs = 480e3 / r2 as f64;
        let mut z = vec![];
        for _ in 0..20 {
            let n = r.read_rx(&mut buf).unwrap();
            z.extend((0..n).map(|k| (buf[2 * k].i as f64, buf[2 * k].q as f64)));
        }
        let (f, a) = tone(&z[z.len() / 2..], fs);
        assert!(
            f.abs() < 2.0 && (a / AMP - 1.0).abs() < 0.02,
            "r2 {r2}: {f} Hz, {}",
            a / AMP
        );
    }
}

#[test]
fn the_tap_gets_frames_and_power_lines() {
    let mut r = BinRx::new(FakeBins::new(), 2).unwrap();
    r.tune_rx(0, 7.074e6).unwrap();
    r.tune_rx(1, 30e6).unwrap(); // bin 1000
    let tap = r.tap();
    let (h, rx) = tap.subscribe();
    // more than the tap may add (TAP_BINS), one of them a channel's
    h.want((990..1060).collect(), 20.0);
    let mut buf = vec![Iq32::default(); 2 * 512];
    r.read_rx(&mut buf).unwrap();
    // the channels' bins, then the tap's from its first on, TAP_BINS more
    let sel = r.inner().selections.last().unwrap().clone();
    assert_eq!(sel.len(), 2 + web888_dsp::binrx::TAP_BINS);
    assert_eq!(sel[0], 236);
    let tap_end = 990 + web888_dsp::binrx::TAP_BINS + 1;
    assert_eq!(sel[1..], (990..tap_end).collect::<Vec<_>>()[..]);
    assert_eq!(r.inner().power_frames, 3000);
    let (mut frames, mut lines) = (0, 0);
    while let Ok(m) = rx.try_recv() {
        match m {
            TapMsg::Frames(f) => {
                assert!(
                    f.iter()
                        .all(|f| f.bins.len() == 2 + web888_dsp::binrx::TAP_BINS)
                );
                frames += f.iter().map(|f| f.count).sum::<usize>();
            }
            TapMsg::Power(p) => {
                assert_eq!(p[TONE_BIN], 1.0);
                lines += 1;
            }
        }
    }
    assert_eq!((frames, lines), (300, 1));
    // the RX channels still right
    assert!(buf[..2 * 200].iter().any(|s| s.i != 0));

    // gone: the selection is the channels' again, no lines
    drop(h);
    r.read_rx(&mut buf).unwrap();
    assert_eq!(r.inner().selections.last().unwrap(), &vec![236, 1000]);
    assert_eq!(r.inner().power_frames, 0);
}

#[test]
fn tap_only_leaves_rx_to_the_radio() {
    let mut r = BinRx::tap_only(FakeBins::new()).unwrap();
    // the radio's own channels and errors (the fake has none)
    assert_eq!(r.caps().rx_channels, 0);
    assert!(matches!(
        r.tune_rx(0, 7.074e6),
        Err(Error::InvalidChannel(0))
    ));
    let tap = r.tap();
    let (h, rx) = tap.subscribe();
    h.want(vec![236], 0.0);
    let mut buf = vec![Iq32::default(); 4];
    // the bins are read for the tap before the radio's RX answers
    assert!(r.read_rx(&mut buf).is_err());
    assert_eq!(r.inner().selections.last().unwrap(), &vec![236]);
    r.read_rx(&mut buf).ok();
    let mut frames = 0;
    while let Ok(TapMsg::Frames(f)) = rx.try_recv() {
        frames += f.iter().map(|f| f.count).sum::<usize>();
    }
    assert!(frames >= 300, "{frames}");
}
