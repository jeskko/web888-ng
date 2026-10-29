// SPDX-License-Identifier: GPL-3.0-or-later
//! RX channels from a channelizer's bins (`doc/m6-channelizer.md`): a
//! [`Radio`] whose channels each take the bin their frequency falls in,
//! shift it to 0 Hz and resample it from the frame rate to the RX rate.
//! Behind it the rest of the server (leases, web, skimmers, HPSDR) works as
//! with tuned RX channels, with as many of them as configured: the limits
//! are the bins a frame carries (64) and the CPU. The waterfall stays the
//! inner radio's.
//!
//! All channels resample in step (one phase schedule), so each bin frame
//! gives every channel the same number of samples and RX frames stay whole.
//! A channel whose bin isn't in a frame (the selection just changed) reads
//! zeros for it.

use crate::C32;
use crate::fir::{kaiser_len, lowpass};
use std::collections::BTreeSet;
use web888_radio::bins::{BinCaps, BinFrame, BinRadio};
use web888_radio::*;

/// Resampler stopband (dB) and passband edge (fraction of the narrower of
/// the two rates; the stopband starts at half of it).
const ATTEN_DB: f64 = 70.0;
const PASS: f64 = 0.375;

/// Rational resampling L/M with one phase schedule for many channels.
struct Rational {
    l: usize,
    m: usize,
    /// `h[p][k]`: phase p's taps, L × the prototype (unity gain).
    h: Vec<Vec<f32>>,
    acc: usize,
}

impl Rational {
    fn new(l: usize, m: usize) -> Self {
        // rates relative to the input: up L, filter, down M
        let narrow = (1.0f64).min(l as f64 / m as f64);
        let fu = l as f64; // the upsampled rate, input = 1
        let tw = (0.5 - PASS) * narrow / fu;
        let n = kaiser_len(ATTEN_DB, tw).max(l);
        let fc = (PASS + 0.5) / 2.0 * narrow / fu;
        let proto = lowpass(n, fc, ATTEN_DB);
        let taps = n.div_ceil(l);
        let h = (0..l)
            .map(|p| {
                (0..taps)
                    .map(|k| proto.get(p + k * l).map_or(0.0, |&x| (x * l as f64) as f32))
                    .collect()
            })
            .collect();
        Rational { l, m, h, acc: 0 }
    }

    fn taps(&self) -> usize {
        self.h[0].len()
    }
}

struct Chan {
    /// The bin and the dial's offset from its centre (Hz).
    tuned: Option<(usize, f64)>,
    rot: num_complex::Complex<f64>,
    step: num_complex::Complex<f64>,
    /// The resampler's input history, newest at `pos - 1`.
    hist: Vec<C32>,
    pos: usize,
}

impl Chan {
    fn new(taps: usize) -> Self {
        Chan {
            tuned: None,
            rot: num_complex::Complex::new(1.0, 0.0),
            step: num_complex::Complex::new(1.0, 0.0),
            hist: vec![C32::new(0.0, 0.0); taps],
            pos: 0,
        }
    }

    fn push(&mut self, x: C32) {
        let n = self.hist.len();
        self.hist[self.pos] = x;
        self.pos = (self.pos + 1) % n;
    }

    /// Σ h[k] · x[newest - k]
    fn dot(&self, h: &[f32]) -> C32 {
        let n = self.hist.len();
        let mut acc = C32::new(0.0, 0.0);
        let mut i = (self.pos + n - 1) % n;
        for &c in h {
            acc += self.hist[i] * c;
            i = if i == 0 { n - 1 } else { i - 1 };
        }
        acc
    }
}

/// RX channels from the bins of `R`.
pub struct BinRx<R> {
    inner: R,
    bins: BinCaps,
    r2: u16,
    rs: Rational,
    ch: Vec<Chan>,
    /// The bins asked for, as last given to `set_bins`.
    selected: Vec<usize>,
    frames: Vec<BinFrame>,
    /// RX frames made but not read yet (whole frames).
    out: Vec<Iq32>,
    out_pos: usize,
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

impl<R: Radio + BinRadio> BinRx<R> {
    /// `channels` RX channels from `inner`'s bins, at the RX rate of
    /// stage-2 decimation 10 (48 kS/s) until `set_rx_rate`.
    pub fn new(mut inner: R, channels: usize) -> Result<Self> {
        let bins = inner.bin_caps();
        if bins.bins == 0 || bins.max_selected == 0 {
            return Err(Error::Unsupported("the radio has no bins"));
        }
        if channels == 0 {
            return Err(Error::InvalidParam("bins mode needs at least one channel"));
        }
        inner.set_bins(&[])?; // the stream starts (frames without bins)
        let mut r = BinRx {
            inner,
            bins,
            r2: 0,
            rs: Rational::new(1, 1),
            ch: Vec::new(),
            selected: Vec::new(),
            frames: Vec::new(),
            out: Vec::new(),
            out_pos: 0,
        };
        r.ch = (0..channels).map(|_| Chan::new(1)).collect();
        r.set_rx_rate(10)?;
        Ok(r)
    }

    pub fn inner(&mut self) -> &mut R {
        &mut self.inner
    }

    fn rx_rate(&self, r2: u16) -> f64 {
        self.inner.adc_hz() / (256.0 * r2 as f64)
    }

    fn reselect(&mut self) -> Result<()> {
        let want: BTreeSet<usize> = self
            .ch
            .iter()
            .filter_map(|c| c.tuned.map(|t| t.0))
            .collect();
        let want: Vec<usize> = want.into_iter().collect();
        if want != self.selected {
            self.inner.set_bins(&want)?;
            self.selected = want;
        }
        Ok(())
    }

    /// Bin frames to RX frames, appended to `out`.
    fn process(&mut self) {
        for f in &self.frames {
            for c in self.ch.iter_mut() {
                let x = c
                    .tuned
                    .and_then(|(b, _)| f.bins.binary_search(&b).ok())
                    .map_or(C32::new(0.0, 0.0), |i| {
                        let s = f.iq[i];
                        let v = num_complex::Complex::new(s.i as f64, s.q as f64) * c.rot;
                        c.rot *= c.step;
                        C32::new(v.re as f32, v.im as f32)
                    });
                c.push(x);
            }
            while self.rs.acc < self.rs.l {
                let h = &self.rs.h[self.rs.acc];
                for c in &self.ch {
                    let y = c.dot(h);
                    self.out.push(Iq32 {
                        i: y.re.round().clamp(i32::MIN as f32, i32::MAX as f32) as i32,
                        q: y.im.round().clamp(i32::MIN as f32, i32::MAX as f32) as i32,
                    });
                }
                self.rs.acc += self.rs.m;
            }
            self.rs.acc -= self.rs.l;
        }
        // keep the rotators on the unit circle
        for c in self.ch.iter_mut() {
            c.rot /= c.rot.norm();
        }
        self.frames.clear();
    }
}

impl<R: Radio + BinRadio> Radio for BinRx<R> {
    fn caps(&self) -> Caps {
        Caps {
            rx_channels: self.ch.len(),
            ..self.inner.caps()
        }
    }

    fn adc_hz(&self) -> f64 {
        self.inner.adc_hz()
    }

    fn set_adc_hz(&mut self, hz: f64) {
        self.inner.set_adc_hz(hz);
    }

    fn set_rx_rate(&mut self, r2: u16) -> Result<()> {
        if !(5..=40).contains(&r2) {
            return Err(Error::InvalidParam("r2 out of range"));
        }
        // RX rate / frame rate as a small fraction (8 / r2 on both backends)
        let ratio = self.rx_rate(r2) / self.bins.frame_rate;
        let (l, m) = (8usize, r2 as usize);
        if ((l as f64 / m as f64) / ratio - 1.0).abs() > 1e-6 {
            return Err(Error::Unsupported(
                "RX rate not 8 / r2 of the bin frame rate",
            ));
        }
        let g = gcd(l, m);
        self.rs = Rational::new(l / g, m / g);
        let taps = self.rs.taps();
        for c in self.ch.iter_mut() {
            let tuned = c.tuned;
            *c = Chan::new(taps);
            c.tuned = tuned;
        }
        for i in 0..self.ch.len() {
            if let Some((b, off)) = self.ch[i].tuned {
                self.set_shift(i, b, off);
            }
        }
        self.out.clear();
        self.out_pos = 0;
        self.r2 = r2;
        Ok(())
    }

    fn rx_r2(&self) -> u16 {
        self.r2
    }

    fn tune_rx(&mut self, ch: usize, f_hz: f64) -> Result<()> {
        if ch >= self.ch.len() {
            return Err(Error::InvalidChannel(ch));
        }
        let (b, off) = self
            .bins
            .bin_of(f_hz)
            .ok_or(Error::InvalidParam("frequency outside the bins"))?;
        if self.ch[ch].tuned.map(|t| t.0) != Some(b) {
            let taps = self.rs.taps();
            self.ch[ch] = Chan::new(taps);
        }
        self.set_shift(ch, b, off);
        self.reselect()
    }

    fn idle_rx(&mut self, ch: usize) -> Result<()> {
        if ch >= self.ch.len() {
            return Err(Error::InvalidChannel(ch));
        }
        self.ch[ch].tuned = None;
        self.reselect()
    }

    fn read_rx(&mut self, buf: &mut [Iq32]) -> Result<usize> {
        let n = self.ch.len();
        if !buf.len().is_multiple_of(n) {
            return Err(Error::BadBuffer);
        }
        if self.out.len() - self.out_pos < buf.len() {
            self.out.drain(..self.out_pos);
            self.out_pos = 0;
            if self.inner.read_bins(&mut self.frames)? > 0 {
                self.process();
            }
        }
        let k = (self.out.len() - self.out_pos).min(buf.len()) / n * n;
        buf[..k].copy_from_slice(&self.out[self.out_pos..self.out_pos + k]);
        self.out_pos += k;
        Ok(k / n)
    }

    fn start_wf(&mut self, slot: usize, f_hz: f64, decim: u16, mode: WfMode) -> Result<()> {
        self.inner.start_wf(slot, f_hz, decim, mode)
    }

    fn read_wf(&mut self, slot: usize, buf: &mut [Iq16]) -> Result<usize> {
        self.inner.read_wf(slot, buf)
    }

    fn wf_gaps(&self, slot: usize) -> u64 {
        self.inner.wf_gaps(slot)
    }

    fn front_end(&mut self, fe: FrontEnd) -> Result<()> {
        self.inner.front_end(fe)
    }

    fn pps(&mut self) -> Result<Option<u32>> {
        self.inner.pps()
    }

    fn hw_stats(&mut self) -> Result<Option<HwStats>> {
        // the inner radio's RX stream (if it has one) isn't what is read
        // here: only its ADC counters apply
        Ok(self.inner.hw_stats()?.map(|s| HwStats {
            adc_overflows: s.adc_overflows,
            ..HwStats::default()
        }))
    }

    fn sample_counter(&mut self) -> Option<u64> {
        self.inner.sample_counter()
    }

    fn identity(&self) -> Identity {
        self.inner.identity()
    }
}

impl<R: Radio + BinRadio> BinRx<R> {
    /// Channel `ch` from bin `b`, its dial `off` Hz from the bin centre.
    fn set_shift(&mut self, ch: usize, b: usize, off: f64) {
        let w = -2.0 * std::f64::consts::PI * off / self.bins.frame_rate;
        let c = &mut self.ch[ch];
        c.tuned = Some((b, off));
        c.step = num_complex::Complex::from_polar(1.0, w);
    }
}
