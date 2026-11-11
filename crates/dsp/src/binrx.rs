// SPDX-License-Identifier: GPL-3.0-or-later
//! RX channels from a channelizer's bins (`doc/m6-channelizer.md`): a
//! [`Radio`] whose channels each take the bin their frequency falls in,
//! shift it to 0 Hz and resample it from the frame rate to the RX rate.
//! Behind it the rest of the server (leases, web, skimmers, HPSDR) works as
//! with tuned RX channels, with as many of them as configured: the limits
//! are the bins a frame carries (64) and the CPU. The waterfall slots stay
//! the inner radio's; waterfalls from the bins go through a [`BinTap`]:
//! power lines, and the frames of the bins a zoomed view wants (selected
//! after the channels' bins, as far as the frame has room): all along, or
//! in bursts of a number of frames (a view needs a few FFTs' worth a
//! line, not every frame: each bin read costs the A9 about 1 % of a
//! core).
//!
//! All channels resample in step (one phase schedule), so each bin frame
//! gives every channel the same number of samples and RX frames stay whole.
//! A channel whose bin isn't in a frame (the selection just changed) reads
//! zeros for it.
//!
//! Zoom jobs (a source with a zoom engine, `hw/docs/channelizer.md` §3.9)
//! go the same way: views hand the tap jobs, the tap gives each a tag
//! (64 outstanding at most) and queues it; `BinRx` submits the queue to
//! the source as it has room and sends each result to the view that owns
//! its tag, moved, not copied. The engine's time is shared by the views
//! through the tap ([`TapHandle::zoom_demand`], [`crate::zoom::schedule`]).
//!
//! [`BinRx::tap_only`]: the radio's own RX channels (the channelizer
//! bitstream's bins tuner, 12 kS/s receivers from the bins in the FPGA);
//! the bins only for the tap.

use crate::C32;
use crate::resample::{History, Rational};
use crate::zoom::{Demand, Grant, schedule};
use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use web888_radio::bins::{BinCaps, BinFrames, BinRadio, ZoomCaps, ZoomJob, ZoomResult};
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
    /// A zoom job's result (the subscriber's tag).
    Zoom(ZoomResult),
}

/// Bins the tap may add to the channels': a 960 kHz view (33 shown, one
/// more run on each side), or two 480 kHz views (19 each).
pub const TAP_BINS: usize = 40;

/// Messages queued per subscriber; a full queue drops (the frames' numbers
/// show the gap).
const TAP_DEPTH: usize = 256;

/// A tag sent to the source without a result this long is free again (a
/// job takes at most 34 ms, a full queue of them about 0.6 s).
const TAG_TIMEOUT: Duration = Duration::from_secs(2);

/// Waterfalls' access to a [`BinRx`]'s bins, from other threads.
#[derive(Clone)]
pub struct BinTap(Arc<Tap>);

struct Tap {
    caps: BinCaps,
    subs: Mutex<TapSubs>,
    changed: AtomicBool,
    zoom: Option<Mutex<ZoomTap>>,
}

/// A tag in use: its subscriber (none: gone, the result is dropped) and
/// when its job went to the source.
#[derive(Clone, Copy)]
struct TagUse {
    owner: Option<u64>,
    sent: Option<Instant>,
}

/// The zoom engine's side of the tap.
struct ZoomTap {
    caps: ZoomCaps,
    tags: Vec<Option<TagUse>>,
    next_tag: usize,
    /// Jobs not yet taken by the source, in order.
    pending: VecDeque<ZoomJob>,
    demands: Vec<(u64, Demand)>,
    grants: Vec<(u64, Grant)>,
    /// Tags freed by [`TAG_TIMEOUT`] (results lost).
    timeouts: u64,
}

impl ZoomTap {
    fn regrant(&mut self, frame_rate: f64) {
        let d: Vec<Demand> = self.demands.iter().map(|(_, d)| d.clone()).collect();
        let g = schedule(&self.caps, frame_rate, &d);
        self.grants = self.demands.iter().map(|(id, _)| *id).zip(g).collect();
    }

    /// Drop `id`'s jobs not yet sent, freeing their tags.
    fn unqueue(&mut self, id: u64) {
        let tags = &mut self.tags;
        self.pending.retain(|j| {
            let t = &mut tags[j.tag as usize];
            let mine = t.is_some_and(|u| u.owner == Some(id));
            if mine {
                *t = None;
            }
            !mine
        });
    }
}

#[derive(Default)]
struct TapSubs {
    next_id: u64,
    list: Vec<TapSub>,
}

struct TapSub {
    id: u64,
    bins: Vec<usize>,
    burst: Option<Burst>,
    power_fps: f64,
    tx: SyncSender<TapMsg>,
}

impl TapSub {
    fn wants_frames(&self) -> bool {
        !self.bins.is_empty() || self.burst.is_some()
    }
}

/// Bins wanted for a number of frames, in groups as the frames have
/// room (a group after the other, in the order given).
struct Burst {
    bins: Vec<usize>,
    frames: usize,
    /// Where the current group starts in `bins`.
    next: usize,
    /// The current group's first bin (the frames read with it count),
    /// once it has room.
    counted_on: Option<usize>,
    got: usize,
}

impl Burst {
    fn rest(&self) -> &[usize] {
        &self.bins[self.next..]
    }
}

impl BinTap {
    /// A tap on a source of `caps`' bins (`BinRx::tap` makes its own).
    pub fn new(caps: BinCaps) -> Self {
        Self::with_zoom(caps, None)
    }

    /// A tap that also carries zoom jobs to an engine of `zoom`'s limits.
    pub fn with_zoom(caps: BinCaps, zoom: Option<ZoomCaps>) -> Self {
        BinTap(Arc::new(Tap {
            caps,
            subs: Mutex::default(),
            changed: AtomicBool::new(false),
            zoom: zoom.map(|caps| {
                Mutex::new(ZoomTap {
                    caps,
                    tags: vec![None; caps.tags],
                    next_tag: 0,
                    pending: VecDeque::new(),
                    demands: Vec::new(),
                    grants: Vec::new(),
                    timeouts: 0,
                })
            }),
        }))
    }

    pub fn caps(&self) -> BinCaps {
        self.0.caps
    }

    /// The zoom engine's limits, if views may use it.
    pub fn zoom_caps(&self) -> Option<ZoomCaps> {
        self.0.zoom.as_ref().map(|z| z.lock().unwrap().caps)
    }

    /// Zoom tags freed without a result.
    pub fn zoom_timeouts(&self) -> u64 {
        self.0
            .zoom
            .as_ref()
            .map_or(0, |z| z.lock().unwrap().timeouts)
    }

    /// The source's side (`BinRx`, on the hw thread): hand the queued
    /// jobs to `submit` (it returns how many the source took); those
    /// leave the queue, their tags' clocks start.
    pub fn zoom_submit(&self, submit: impl FnOnce(&[ZoomJob]) -> Result<usize>) -> Result<()> {
        let Some(z) = &self.0.zoom else { return Ok(()) };
        let mut z = z.lock().unwrap();
        if z.pending.is_empty() {
            return Ok(());
        }
        let k = submit(z.pending.make_contiguous())?;
        let now = Instant::now();
        let k = k.min(z.pending.len());
        let ZoomTap { pending, tags, .. } = &mut *z;
        for j in pending.drain(..k) {
            if let Some(t) = &mut tags[j.tag as usize] {
                t.sent = Some(now);
            }
        }
        Ok(())
    }

    /// The source's side: results to their tags' owners (moved); tags
    /// sent too long ago freed.
    pub fn zoom_deliver(&self, results: &mut Vec<ZoomResult>) {
        let Some(z) = &self.0.zoom else { return };
        let mut owners = Vec::with_capacity(results.len());
        {
            let mut z = z.lock().unwrap();
            for r in results.iter() {
                let t = z.tags.get_mut(r.tag as usize).and_then(Option::take);
                owners.push(t.and_then(|t| t.owner));
            }
            let now = Instant::now();
            let mut lost = 0;
            for t in &mut z.tags {
                if t.is_some_and(|u| u.sent.is_some_and(|s| now > s + TAG_TIMEOUT)) {
                    *t = None;
                    lost += 1;
                }
            }
            z.timeouts += lost;
        }
        if results.is_empty() {
            return;
        }
        let g = self.0.subs.lock().unwrap();
        for (r, o) in results.drain(..).zip(owners) {
            if let Some(s) = o.and_then(|o| g.list.iter().find(|s| s.id == o)) {
                let _ = s.tx.try_send(TapMsg::Zoom(r));
            }
        }
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
            burst: None,
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

    /// Bins wanted after the channels' (in subscription order: each
    /// subscriber's all-along bins, then its burst's) and the power line
    /// rate (the fastest wanted; 0: none).
    fn wants(&self) -> (Vec<usize>, f64) {
        let g = self.0.subs.lock().unwrap();
        let bins = g
            .list
            .iter()
            .flat_map(|s| {
                let b = s.burst.as_ref().map_or(&[][..], Burst::rest);
                s.bins.iter().chain(b).copied()
            })
            .collect();
        let fps = g.list.iter().map(|s| s.power_fps).fold(0.0, f64::max);
        (bins, fps)
    }

    /// The selection is `sel`: each burst's group is what of its next
    /// bins it has, counted on the first (none: it waits for room).
    fn granted(&self, sel: &[usize]) {
        let mut g = self.0.subs.lock().unwrap();
        for b in g.list.iter_mut().filter_map(|s| s.burst.as_mut()) {
            let first = b.rest().first().copied();
            let has = first.filter(|k| sel.binary_search(k).is_ok());
            if has != b.counted_on {
                b.counted_on = has;
                b.got = 0;
            }
        }
    }

    /// Frames read: the bursts count them; a group done gives way to the
    /// next, the last ends the burst (the selection then changes).
    fn count(&self, frames: &[BinFrames], sel: &[usize]) {
        let mut g = self.0.subs.lock().unwrap();
        let mut done = false;
        for s in &mut g.list {
            let Some(b) = &mut s.burst else { continue };
            let Some(k) = b.counted_on else { continue };
            b.got += frames
                .iter()
                .filter(|f| f.bins.binary_search(&k).is_ok())
                .map(|f| f.count)
                .sum::<usize>();
            if b.got >= b.frames {
                // the group: the bins from `next` on that were selected
                let n = b
                    .rest()
                    .iter()
                    .take_while(|k| sel.binary_search(k).is_ok())
                    .count()
                    .max(1);
                b.next += n;
                b.counted_on = None;
                b.got = 0;
                if b.next >= b.bins.len() {
                    s.burst = None;
                }
                done = true;
            }
        }
        if done {
            self.0.changed.store(true, Ordering::Relaxed);
        }
    }

    /// Whether a subscriber wants bins.
    pub fn wants_frames(&self) -> bool {
        self.0
            .subs
            .lock()
            .unwrap()
            .list
            .iter()
            .any(TapSub::wants_frames)
    }

    /// Frames to the subscribers wanting bins.
    pub fn send_frames(&self, f: Arc<Vec<BinFrames>>) {
        self.send(|| TapMsg::Frames(f.clone()), TapSub::wants_frames);
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

    /// Want `bins` for `frames` frames read with them, on top of the
    /// all-along ones; as many at a time as the frames have room for, a
    /// group after the other. A burst not done yet goes on (`restart`:
    /// this one replaces it). Empty `bins`: none.
    pub fn burst(&self, bins: Vec<usize>, frames: usize, restart: bool) {
        let t = &self.tap.0;
        let mut g = t.subs.lock().unwrap();
        let Some(s) = g.list.iter_mut().find(|s| s.id == self.id) else {
            return;
        };
        if bins.is_empty() || frames == 0 {
            s.burst = None;
        } else if restart || s.burst.is_none() {
            s.burst = Some(Burst {
                bins,
                frames,
                next: 0,
                counted_on: None,
                got: 0,
            });
        } else {
            return;
        }
        t.changed.store(true, Ordering::Relaxed);
    }
}

impl TapHandle {
    /// The view's demand on the zoom engine (none: it uses none); the
    /// grants of all are worked out again.
    pub fn zoom_demand(&self, d: Option<Demand>) {
        let t = &self.tap.0;
        let Some(z) = &t.zoom else { return };
        let mut z = z.lock().unwrap();
        let had = z.demands.iter().position(|(id, _)| *id == self.id);
        match (had, d) {
            (Some(k), Some(d)) if z.demands[k].1 == d => return,
            (Some(k), Some(d)) => z.demands[k].1 = d,
            (None, Some(d)) => z.demands.push((self.id, d)),
            (Some(k), None) => {
                z.demands.remove(k);
            }
            (None, None) => return,
        }
        z.regrant(t.caps.frame_rate);
    }

    /// The view's share of the zoom engine, if it asked for one.
    pub fn zoom_grant(&self) -> Option<Grant> {
        let z = self.tap.0.zoom.as_ref()?.lock().unwrap();
        z.grants.iter().find(|(id, _)| *id == self.id).map(|g| g.1)
    }

    /// Queue `jobs` (their tags are given here), after dropping this
    /// view's jobs still queued if `replace`; returns the tags of the
    /// ones queued, in order (fewer when the tags run out).
    pub fn submit_zoom(&self, jobs: &[ZoomJob], replace: bool) -> Vec<u8> {
        let Some(z) = &self.tap.0.zoom else {
            return Vec::new();
        };
        let mut z = z.lock().unwrap();
        if replace {
            z.unqueue(self.id);
        }
        let ntags = z.tags.len();
        let mut tags = Vec::with_capacity(jobs.len());
        for j in jobs {
            // the next free tag after the last given (the longest free)
            let Some(t) = (0..ntags)
                .map(|k| (z.next_tag + k) % ntags)
                .find(|&t| z.tags[t].is_none())
            else {
                break;
            };
            z.tags[t] = Some(TagUse {
                owner: Some(self.id),
                sent: None,
            });
            z.next_tag = (t + 1) % ntags;
            z.pending.push_back(ZoomJob { tag: t as u8, ..*j });
            tags.push(t as u8);
        }
        tags
    }
}

impl Drop for TapHandle {
    fn drop(&mut self) {
        let t = &self.tap.0;
        if let Some(z) = &t.zoom {
            let mut z = z.lock().unwrap();
            z.unqueue(self.id);
            // results of jobs sent are dropped when they come
            for u in z.tags.iter_mut().flatten() {
                if u.owner == Some(self.id) {
                    u.owner = None;
                }
            }
            if let Some(k) = z.demands.iter().position(|(id, _)| *id == self.id) {
                z.demands.remove(k);
                z.regrant(t.caps.frame_rate);
            }
        }
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
    /// RX from the inner radio (its bins tuner), the bins only for the tap.
    tap_only: bool,
    /// Frames per power line (0: none).
    power_frames: u16,
    power: Vec<f32>,
    /// Views may use the source's zoom engine (if it has one).
    zoom: bool,
    zoom_out: Vec<ZoomResult>,
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
            tap_only: false,
            power_frames: 0,
            power: Vec::new(),
            zoom: true,
            zoom_out: Vec::new(),
        };
        r.ch = (0..channels).map(|_| Chan::new(1)).collect();
        r.set_rx_rate(10)?;
        Ok(r)
    }

    /// `inner`'s own RX channels (the channelizer bitstream's bins tuner),
    /// its bins only for the tap: the waterfall.
    pub fn tap_only(mut inner: R) -> Result<Self> {
        let bins = inner.bin_caps();
        if bins.bins == 0 || bins.max_selected == 0 {
            return Err(Error::Unsupported("the radio has no bins"));
        }
        inner.set_bins(&[])?;
        Ok(BinRx {
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
            tap_only: true,
            power_frames: 0,
            power: Vec::new(),
            zoom: true,
            zoom_out: Vec::new(),
        })
    }

    pub fn inner(&mut self) -> &mut R {
        &mut self.inner
    }

    fn rx_rate(&self, r2: u16) -> f64 {
        self.inner.adc_hz() / (256.0 * r2 as f64)
    }

    /// Whether views may use the source's zoom engine (default: yes);
    /// set before [`tap`](Self::tap).
    pub fn set_zoom_engine(&mut self, on: bool) {
        self.zoom = on;
    }

    /// The tap for waterfalls from the bins (with the source's zoom
    /// engine, if it has one and it is on).
    pub fn tap(&mut self) -> BinTap {
        let caps = self.bins;
        let zoom = if self.zoom {
            self.inner.zoom_caps()
        } else {
            None
        };
        self.tap
            .get_or_insert_with(|| BinTap::with_zoom(caps, zoom))
            .clone()
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
        if let Some(t) = &self.tap {
            t.granted(&self.selected);
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
        if t.0.zoom.is_some() {
            let inner = &mut self.inner;
            t.zoom_submit(|jobs| inner.submit_zoom(jobs))?;
            inner.read_zoom(&mut self.zoom_out)?;
            t.zoom_deliver(&mut self.zoom_out);
        }
        if self.frames.is_empty() {
            return Ok(());
        }
        // the bursts end after their last frames went out
        let frames = Arc::new(std::mem::take(&mut self.frames));
        if t.wants_frames() {
            t.send_frames(frames.clone());
        }
        t.count(&frames, &self.selected);
        Ok(())
    }
}

impl<R: Radio + BinRadio> Radio for BinRx<R> {
    fn caps(&self) -> Caps {
        if self.tap_only {
            return self.inner.caps();
        }
        Caps {
            rx_channels: self.ch.len(),
            rx_r2: 5..=40,
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
        if self.tap_only {
            return self.inner.set_rx_rate(r2);
        }
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
        if self.tap_only {
            return self.inner.rx_r2();
        }
        self.r2
    }

    fn tune_rx(&mut self, ch: usize, f_hz: f64) -> Result<()> {
        if self.tap_only {
            return self.inner.tune_rx(ch, f_hz);
        }
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
        if self.tap_only {
            return self.inner.idle_rx(ch);
        }
        if ch >= self.ch.len() {
            return Err(Error::InvalidChannel(ch));
        }
        let taps = self.taps();
        self.ch[ch] = Chan::new(taps);
        self.map = None;
        self.reselect()
    }

    fn read_rx(&mut self, buf: &mut [Iq32]) -> Result<usize> {
        if self.tap_only {
            // the bins stream read as it comes, for the tap (and so its
            // ring doesn't lap), then the RX
            self.tap_changes()?;
            self.inner.read_bins(&mut self.frames)?;
            self.tap_send()?;
            return self.inner.read_rx(buf);
        }
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
        if self.tap_only {
            return self.inner.hw_stats();
        }
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
