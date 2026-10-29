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
use crate::resample::{History, Rational};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use web888_radio::bins::{BinCaps, BinFrames, BinRadio};
use web888_radio::*;

struct Chan {
    /// The bin and the dial's offset from its centre (Hz).
    tuned: Option<(usize, f64)>,
    rot: C32,
    step: C32,
    hist: History,
}

impl Chan {
    fn new(taps: usize) -> Self {
        Chan {
            tuned: None,
            rot: C32::new(1.0, 0.0),
            step: C32::new(1.0, 0.0),
            hist: History::new(taps),
        }
    }
}

/// What a [`BinTap`] subscriber gets.
pub enum TapMsg {
    /// Bin frames, as read (every selected bin, not only the wanted ones).
    Frames(Arc<Vec<BinFrames>>),
    /// A power line: every bin's mean |X|² (a full-scale sine reads 1).
    Power(Arc<Vec<f32>>),
}

/// Bins the tap may add to the channels' (a 480 kHz view takes 17).
pub const TAP_BINS: usize = 17;

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
    pub fn send_frames(&self, f: Arc<Vec<BinFrames>>) {
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

/// A frame's bin list and each channel's index in it.
type BinIndex = (Arc<[usize]>, Vec<Option<usize>>);

/// RX channels from the bins of `R`.
pub struct BinRx<R> {
    inner: R,
    bins: BinCaps,
    r2: u16,
    /// RX rate / frame rate; none at 1 (the shifted bin is the channel).
    rs: Option<Rational>,
    ch: Vec<Chan>,
    /// The bins asked for, as last given to `set_bins`.
    selected: Vec<usize>,
    frames: Vec<BinFrames>,
    /// RX frames made but not read yet (whole frames).
    out: Vec<Iq32>,
    out_pos: usize,
    /// The frames' bin list last seen and each channel's index in it
    /// (cleared when a channel's bin changes).
    map: Option<BinIndex>,
    tap: Option<BinTap>,
    /// Frames per power line (0: none).
    power_frames: u16,
    power: Vec<f32>,
}

/// `as` saturates; round half away from zero without libm's roundf (a
/// call per sample on musl).
fn to_iq32(y: C32) -> Iq32 {
    Iq32 {
        i: (y.re + 0.5f32.copysign(y.re)) as i32,
        q: (y.im + 0.5f32.copysign(y.im)) as i32,
    }
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
            rs: None,
            ch: Vec::new(),
            selected: Vec::new(),
            frames: Vec::new(),
            out: Vec::new(),
            out_pos: 0,
            map: None,
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
        // the tap's bins where the frame has room, at most TAP_BINS more
        // (each bin read costs the hw thread; the views fall back on the
        // power line)
        if let Some(t) = &self.tap {
            let room = (want.len() + TAP_BINS).min(self.bins.max_selected);
            for b in t.wants().0 {
                if want.len() >= room {
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
        let BinRx {
            frames,
            ch,
            rs,
            out,
            map,
            ..
        } = self;
        let nch = ch.len();
        for f in frames.iter() {
            // each channel's index in the frame, found once per selection
            if !map.as_ref().is_some_and(|(b, _)| Arc::ptr_eq(b, &f.bins)) {
                let idx = ch
                    .iter()
                    .map(|c| c.tuned.and_then(|(b, _)| f.bins.binary_search(&b).ok()))
                    .collect();
                *map = Some((f.bins.clone(), idx));
            }
            let idx = &map.as_ref().expect("set").1;
            let n = f.bins.len();
            // idle channels cost nothing: they read zeros, and a channel
            // starts from a clear history when it is tuned
            let Some(rs) = rs else {
                // at the frame rate: the shifted samples themselves, a
                // channel at a time
                let base = out.len();
                out.resize(base + f.count * nch, Iq32::default());
                for (j, (c, &i)) in ch.iter_mut().zip(idx).enumerate() {
                    let Some(i) = i.filter(|_| c.tuned.is_some()) else {
                        continue;
                    };
                    let (mut rot, step) = (c.rot, c.step);
                    for k in 0..f.count {
                        let s = f.iq[k * n + i];
                        out[base + k * nch + j] = to_iq32(C32::new(s.i as f32, s.q as f32) * rot);
                        rot *= step;
                    }
                    c.rot = rot;
                }
                continue;
            };
            for k in 0..f.count {
                let row = f.frame(k);
                for (c, &i) in ch.iter_mut().zip(idx) {
                    if c.tuned.is_none() {
                        continue;
                    }
                    let x = i.map_or(C32::new(0.0, 0.0), |i| {
                        let s = row[i];
                        let v = C32::new(s.i as f32, s.q as f32) * c.rot;
                        c.rot *= c.step;
                        v
                    });
                    c.hist.push(x);
                }
                rs.step(|h| {
                    out.extend(ch.iter().map(|c| {
                        if c.tuned.is_none() {
                            Iq32::default()
                        } else {
                            to_iq32(c.hist.dot(h))
                        }
                    }))
                });
            }
        }
        // keep the rotators on the unit circle
        for c in ch.iter_mut() {
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
        self.rs = (l != m).then(|| Rational::new(l / g, m / g));
        let taps = self.taps();
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
            let taps = self.taps();
            self.ch[ch] = Chan::new(taps);
        }
        self.set_shift(ch, b, off);
        self.reselect()
    }

    fn idle_rx(&mut self, ch: usize) -> Result<()> {
        if ch >= self.ch.len() {
            return Err(Error::InvalidChannel(ch));
        }
        let taps = self.taps();
        self.ch[ch] = Chan::new(taps);
        self.map = None;
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
    fn taps(&self) -> usize {
        self.rs.as_ref().map_or(1, Rational::taps)
    }

    /// Channel `ch` from bin `b`, its dial `off` Hz from the bin centre.
    fn set_shift(&mut self, ch: usize, b: usize, off: f64) {
        let w = -2.0 * std::f64::consts::PI * off / self.bins.frame_rate;
        self.map = None;
        let c = &mut self.ch[ch];
        c.tuned = Some((b, off));
        let s = num_complex::Complex::<f64>::from_polar(1.0, w);
        c.step = C32::new(s.re as f32, s.im as f32);
    }
}
