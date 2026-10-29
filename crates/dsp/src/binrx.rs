// SPDX-License-Identifier: GPL-3.0-or-later
//! RX channels from a channelizer's bins (`doc/m6-channelizer.md`): a
//! [`Radio`] whose channels each take the bin their frequency falls in,
//! shift it to 0 Hz and resample it from the frame rate to the RX rate.
//! Behind it the rest of the server (leases, web, skimmers, HPSDR) works as
//! with tuned RX channels, with as many of them as configured: the limits
//! are the bins a frame carries (64) and the CPU. The waterfall slots stay
//! the inner radio's; waterfalls from the bins go through a [`BinTap`]:
//! power lines, and the frames of the bins a zoomed view wants (selected
//! after the channels' bins, as far as the frame has room).
//!
//! All channels resample in step (one phase schedule), so each bin frame
//! gives every channel the same number of samples and RX frames stay whole.
//! A channel whose bin isn't in a frame (the selection just changed) reads
//! zeros for it.

use crate::C32;
use crate::fir::{kaiser_len, lowpass};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use web888_radio::bins::{BinCaps, BinFrame, BinRadio};
use web888_radio::*;

/// Resampler stopband (dB) and passband edge (fraction of the narrower of
/// the two rates). The stopband starts at 1 - PASS of it: what folds back
/// from the transition band lands above the passband edge, not in it, and
/// the wide transition halves the taps (the cost on the A9).
const ATTEN_DB: f64 = 70.0;
const PASS: f64 = 0.375;

/// Rational resampling L/M with one phase schedule for many channels.
struct Rational {
    l: usize,
    m: usize,
    /// Phase p's taps, L × the prototype (unity gain), oldest sample's
    /// first: `h[p][j]` weighs the j-th of the history window.
    h: Vec<Vec<f32>>,
    acc: usize,
}

impl Rational {
    fn new(l: usize, m: usize) -> Self {
        // rates relative to the input: up L, filter, down M
        let narrow = (1.0f64).min(l as f64 / m as f64);
        let fu = l as f64; // the upsampled rate, input = 1
        let tw = (1.0 - 2.0 * PASS) * narrow / fu;
        let n = kaiser_len(ATTEN_DB, tw).max(l);
        let fc = 0.5 * narrow / fu;
        let proto = lowpass(n, fc, ATTEN_DB);
        let taps = n.div_ceil(l);
        let h = (0..l)
            .map(|p| {
                (0..taps)
                    .rev()
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
    rot: C32,
    step: C32,
    /// The resampler's input history, twice in a row so that the last
    /// `n` samples are one slice, `[pos, pos + n)`, oldest first.
    re: Vec<f32>,
    im: Vec<f32>,
    pos: usize,
}

impl Chan {
    fn new(taps: usize) -> Self {
        Chan {
            tuned: None,
            rot: C32::new(1.0, 0.0),
            step: C32::new(1.0, 0.0),
            re: vec![0.0; 2 * taps],
            im: vec![0.0; 2 * taps],
            pos: 0,
        }
    }

    fn push(&mut self, x: C32) {
        let n = self.re.len() / 2;
        self.re[self.pos] = x.re;
        self.re[self.pos + n] = x.re;
        self.im[self.pos] = x.im;
        self.im[self.pos + n] = x.im;
        self.pos = if self.pos + 1 == n { 0 } else { self.pos + 1 };
    }

    /// The window (oldest first) weighed by `h`.
    fn dot(&self, h: &[f32]) -> C32 {
        let n = h.len();
        let (re, im) = (
            &self.re[self.pos..self.pos + n],
            &self.im[self.pos..self.pos + n],
        );
        // four partial sums: no dependency chain on one accumulator
        let (mut r, mut i) = ([0f32; 4], [0f32; 4]);
        for ((h, re), im) in h
            .as_chunks::<4>()
            .0
            .iter()
            .zip(re.as_chunks::<4>().0)
            .zip(im.as_chunks::<4>().0)
        {
            for k in 0..4 {
                r[k] += h[k] * re[k];
                i[k] += h[k] * im[k];
            }
        }
        let tail = n / 4 * 4;
        let (mut rt, mut it) = (r.iter().sum::<f32>(), i.iter().sum::<f32>());
        for k in tail..n {
            rt += h[k] * re[k];
            it += h[k] * im[k];
        }
        C32::new(rt, it)
    }
}

/// What a [`BinTap`] subscriber gets.
pub enum TapMsg {
    /// Bin frames, as read (every selected bin, not only the wanted ones).
    Frames(Arc<Vec<BinFrame>>),
    /// A power line: every bin's mean |X|² (a full-scale sine reads 1).
    Power(Arc<Vec<f32>>),
}

/// Messages queued per subscriber; a full queue drops (the frames' numbers
/// show the gap).
const TAP_DEPTH: usize = 256;

/// Waterfalls' access to a [`BinRx`]'s bins, from other threads.
#[derive(Clone)]
pub struct BinTap(Arc<Tap>);

struct Tap {
    caps: BinCaps,
    subs: Mutex<TapSubs>,
    changed: AtomicBool,
}

#[derive(Default)]
struct TapSubs {
    next_id: u64,
    list: Vec<TapSub>,
}

struct TapSub {
    id: u64,
    bins: Vec<usize>,
    power_fps: f64,
    tx: SyncSender<TapMsg>,
}

impl BinTap {
    /// A tap on a source of `caps`' bins (`BinRx::tap` makes its own).
    pub fn new(caps: BinCaps) -> Self {
        BinTap(Arc::new(Tap {
            caps,
            subs: Mutex::default(),
            changed: AtomicBool::new(false),
        }))
    }

    pub fn caps(&self) -> BinCaps {
        self.0.caps
    }

    /// A new subscriber, wanting nothing until [`TapHandle::want`].
    pub fn subscribe(&self) -> (TapHandle, Receiver<TapMsg>) {
        let (tx, rx) = sync_channel(TAP_DEPTH);
        let mut g = self.0.subs.lock().unwrap();
        let id = g.next_id;
        g.next_id += 1;
        g.list.push(TapSub {
            id,
            bins: Vec::new(),
            power_fps: 0.0,
            tx,
        });
        (
            TapHandle {
                tap: self.clone(),
                id,
            },
            rx,
        )
    }

    /// Bins wanted after the channels' (in subscription order) and the
    /// power line rate (the fastest wanted; 0: none).
    fn wants(&self) -> (Vec<usize>, f64) {
        let g = self.0.subs.lock().unwrap();
        let bins = g.list.iter().flat_map(|s| s.bins.iter().copied()).collect();
        let fps = g.list.iter().map(|s| s.power_fps).fold(0.0, f64::max);
        (bins, fps)
    }

    /// Whether a subscriber wants bins.
    pub fn wants_frames(&self) -> bool {
        self.0
            .subs
            .lock()
            .unwrap()
            .list
            .iter()
            .any(|s| !s.bins.is_empty())
    }

    /// Frames to the subscribers wanting bins.
    pub fn send_frames(&self, f: Arc<Vec<BinFrame>>) {
        self.send(|| TapMsg::Frames(f.clone()), |s| !s.bins.is_empty());
    }

    /// A power line to the subscribers wanting them.
    pub fn send_power(&self, line: Arc<Vec<f32>>) {
        self.send(|| TapMsg::Power(line.clone()), |s| s.power_fps > 0.0);
    }

    /// Send to the subscribers `pick` chooses; drops the ones gone.
    fn send(&self, msg: impl Fn() -> TapMsg, pick: impl Fn(&TapSub) -> bool) {
        let mut g = self.0.subs.lock().unwrap();
        let before = g.list.len();
        g.list.retain(|s| {
            !pick(s) || !matches!(s.tx.try_send(msg()), Err(TrySendError::Disconnected(_)))
        });
        if g.list.len() != before {
            self.0.changed.store(true, Ordering::Relaxed);
        }
    }
}

/// A subscription; dropping it unsubscribes.
pub struct TapHandle {
    tap: BinTap,
    id: u64,
}

impl TapHandle {
    /// Want `bins` (frames carrying them) and power lines at `power_fps`
    /// lines a second (0: none).
    pub fn want(&self, bins: Vec<usize>, power_fps: f64) {
        let t = &self.tap.0;
        let mut g = t.subs.lock().unwrap();
        if let Some(s) = g.list.iter_mut().find(|s| s.id == self.id) {
            s.bins = bins;
            s.power_fps = power_fps;
        }
        t.changed.store(true, Ordering::Relaxed);
    }
}

impl Drop for TapHandle {
    fn drop(&mut self) {
        let t = &self.tap.0;
        t.subs.lock().unwrap().list.retain(|s| s.id != self.id);
        t.changed.store(true, Ordering::Relaxed);
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
    tap: Option<BinTap>,
    /// Frames per power line (0: none).
    power_frames: u16,
    power: Vec<f32>,
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
            tap: None,
            power_frames: 0,
            power: Vec::new(),
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

    /// The tap for waterfalls from the bins.
    pub fn tap(&mut self) -> BinTap {
        let caps = self.bins;
        self.tap.get_or_insert_with(|| BinTap::new(caps)).clone()
    }

    fn reselect(&mut self) -> Result<()> {
        let mut want: BTreeSet<usize> = self
            .ch
            .iter()
            .filter_map(|c| c.tuned.map(|t| t.0))
            .collect();
        // the tap's bins where the frame has room
        if let Some(t) = &self.tap {
            for b in t.wants().0 {
                if want.len() >= self.bins.max_selected {
                    break;
                }
                if b < self.bins.bins {
                    want.insert(b);
                }
            }
        }
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
            // idle channels cost nothing: they read zeros, and a channel
            // starts from a clear history when it is tuned
            for c in self.ch.iter_mut() {
                let Some((b, _)) = c.tuned else { continue };
                let x = f
                    .bins
                    .binary_search(&b)
                    .ok()
                    .map_or(C32::new(0.0, 0.0), |i| {
                        let s = f.iq[i];
                        let v = C32::new(s.i as f32, s.q as f32) * c.rot;
                        c.rot *= c.step;
                        v
                    });
                c.push(x);
            }
            while self.rs.acc < self.rs.l {
                let h = &self.rs.h[self.rs.acc];
                for c in &self.ch {
                    if c.tuned.is_none() {
                        self.out.push(Iq32::default());
                        continue;
                    }
                    let y = c.dot(h);
                    // `as` saturates; round half away from zero without
                    // libm's roundf (a call per sample on musl)
                    self.out.push(Iq32 {
                        i: (y.re + 0.5f32.copysign(y.re)) as i32,
                        q: (y.im + 0.5f32.copysign(y.im)) as i32,
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
    }

    /// The tap's changes: its bins, its power line rate.
    fn tap_changes(&mut self) -> Result<()> {
        let Some(t) = &self.tap else { return Ok(()) };
        if !t.0.changed.swap(false, Ordering::Relaxed) {
            return Ok(());
        }
        let fps = t.wants().1;
        let frames = if fps > 0.0 {
            (self.bins.frame_rate / fps)
                .round()
                .clamp(1.0, u16::MAX as f64) as u16
        } else {
            0
        };
        if frames != self.power_frames {
            self.inner.set_power(frames)?;
            self.power_frames = frames;
        }
        self.reselect()
    }

    /// The frames read to the subscribers wanting bins, the power lines to
    /// those wanting them.
    fn tap_send(&mut self) -> Result<()> {
        let Some(t) = &self.tap else {
            self.frames.clear();
            return Ok(());
        };
        if self.power_frames > 0 {
            while self.inner.read_power(&mut self.power)?.is_some() {
                t.send_power(Arc::new(std::mem::take(&mut self.power)));
            }
        }
        if self.frames.is_empty() {
            return Ok(());
        }
        if t.wants_frames() {
            t.send_frames(Arc::new(std::mem::take(&mut self.frames)));
        } else {
            self.frames.clear();
        }
        Ok(())
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
        let taps = self.rs.taps();
        self.ch[ch] = Chan::new(taps);
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
            self.tap_changes()?;
            if self.inner.read_bins(&mut self.frames)? > 0 {
                self.process();
            }
            self.tap_send()?;
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
        let s = num_complex::Complex::<f64>::from_polar(1.0, w);
        c.step = C32::new(s.re as f32, s.im as f32);
    }
}
