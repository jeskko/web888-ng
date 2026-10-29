// SPDX-License-Identifier: GPL-3.0-or-later
//! Waterfall views from a channelizer's bins (`doc/m6-channelizer.md` §4),
//! for radios without WF slots in bins mode:
//!
//! * a **zoomed** view (at most [`ZOOM_BINS`] bins) takes the bins it
//!   covers through a [`BinTap`] and puts each through its own FFT; the
//!   middle 30 kHz of each (the bins are 60 kS/s, 30 kHz apart, flat well
//!   past ±15 kHz) side by side tile the view without a seam;
//! * a **wide** view draws the power line: 30 kHz per bin, merged into
//!   the view's display bins.
//!
//! A zoomed view's bins come after the RX channels' in the selection; a
//! bin the frame had no room for shows the power line's level instead.

use crate::workers::{ViewCmd, ViewSub, period, rebin, shed};
use crossbeam_channel::{Receiver as CbReceiver, TryRecvError};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};
use web888_core::pool::{Ctx, Job};
use web888_dsp::binrx::{BinTap, TapHandle, TapMsg};
use web888_dsp::chan::Droop;
use web888_dsp::rx_to_c32;
use web888_dsp::wf::{WF_DB0, WfEngine, WfSpec};
use web888_radio::bins::{BinCaps, BinFrames};

/// Most bins a zoomed view shows (a 960 kHz span at 30 kHz: 33); wider
/// views draw the power line. All views together get `binrx::TAP_BINS`
/// beyond the channels' (each bin read costs the A9 about 1 % of a core,
/// memory-bound; a view's FFTs about 9 % whatever its span), from the
/// view's centre out: bins left over show the power line, interpolated.
pub const ZOOM_BINS: usize = 33;

/// Bins a zoomed view also runs on each side of the ones it shows, so a
/// view dragged across a bin edge has the new bin's spectrum at hand.
const MARGIN: usize = 1;

/// After a move or a zoom, how long a view holds its lines while bins
/// new to it fill their FFTs (instead of drawing the power line there).
const SETTLE: Duration = Duration::from_millis(250);

/// How long a bin missing from the frames (the CPU busy, the tap's queue
/// full) shows its last spectrum before the power line's level.
const HOLD: Duration = Duration::from_secs(2);

struct Sub {
    s: ViewSub,
    next: Instant,
}

/// The frames' bin list last seen and each engine's index in it.
type BinMap = (Arc<[usize]>, Vec<Option<usize>>);

/// One bin's FFT.
struct Eng {
    bin: usize,
    e: WfEngine,
    /// The bin's last frame.
    last: Option<u64>,
    /// Its last spectrum (the view's part) and when it came.
    held: Option<(Vec<f32>, Instant)>,
}

/// The per-bin FFTs of a zoomed view: the bins it shows and its margin,
/// by frequency.
struct Zoom {
    n: usize,
    fps: f64,
    engs: Vec<Eng>,
    map: Option<BinMap>,
}

impl Zoom {
    /// Run `bins`, keeping the FFTs of those already running.
    fn keep(&mut self, bins: &[usize], spec: WfSpec) {
        let mut old = std::mem::take(&mut self.engs);
        self.engs = bins
            .iter()
            .map(|&bin| match old.iter().position(|e| e.bin == bin) {
                Some(k) => old.swap_remove(k),
                None => Eng {
                    bin,
                    e: WfEngine::with_fft_size(spec, self.n),
                    last: None,
                    held: None,
                },
            })
            .collect();
        self.map = None;
    }

    fn frames(&mut self, runs: &[BinFrames]) {
        for f in runs {
            if !self
                .map
                .as_ref()
                .is_some_and(|(b, _)| Arc::ptr_eq(b, &f.bins))
            {
                let idx = self
                    .engs
                    .iter()
                    .map(|e| f.bins.binary_search(&e.bin).ok())
                    .collect();
                self.map = Some((f.bins.clone(), idx));
            }
            let (_, idx) = self.map.as_ref().expect("set");
            let n = f.bins.len();
            for (e, j) in self.engs.iter_mut().zip(idx) {
                let Some(j) = *j else { continue };
                let contiguous = e.last.is_some_and(|l| l + 1 == f.first);
                let x = (0..f.count).map(|k| rx_to_c32(f.iq[k * n + j]));
                e.e.push_c32(x, contiguous);
                e.last = Some(f.end() - 1);
            }
        }
    }
}

/// One view from the bins, shared by its sessions; its only session may
/// move it ([`ViewCmd::Move`]). Ends when the service drops its command
/// sender (the view is gone).
pub struct BinViewJob {
    center_hz: f64,
    span_hz: f64,
    budget: usize,
    caps: BinCaps,
    cmd: CbReceiver<ViewCmd>,
    subs: Vec<Sub>,
    tap: TapHandle,
    rx: Receiver<TapMsg>,
    /// The bins the view covers, by frequency.
    view: Vec<usize>,
    zoom: Option<Zoom>,
    /// Power lines since the last line (summed), and the last line's
    /// levels (dB): a wide view's line, a zoomed view's missing bins.
    psum: Vec<f64>,
    plines: usize,
    plast: Option<Vec<f32>>,
    /// The last power line, for the levels of a moved view's new bins.
    praw: Option<Arc<Vec<f32>>>,
    /// The last move or zoom.
    moved: Instant,
    next: Instant,
}

/// The bins whose 30 kHz around the centre meet `lo..hi`, by frequency.
pub fn view_bins(caps: &BinCaps, lo: f64, hi: f64) -> Vec<usize> {
    let h = caps.bin_hz / 2.0;
    let mut v: Vec<usize> = (0..caps.bins)
        .filter(|&k| {
            let f = caps.freq(k);
            f + h > lo && f - h < hi
        })
        .collect();
    v.sort_by(|&a, &b| caps.freq(a).total_cmp(&caps.freq(b)));
    v
}

fn db(p: f64) -> f32 {
    (10.0 * p.max(1e-30).log10()) as f32
}

impl BinViewJob {
    pub fn new(
        center_hz: f64,
        span_hz: f64,
        budget: usize,
        tap: &BinTap,
        cmd: CbReceiver<ViewCmd>,
    ) -> Self {
        let caps = tap.caps();
        let (handle, rx) = tap.subscribe();
        BinViewJob {
            center_hz,
            span_hz,
            budget: budget.max(1),
            caps,
            cmd,
            subs: Vec::new(),
            tap: handle,
            rx,
            psum: Vec::new(),
            view: Vec::new(),
            zoom: None,
            plines: 0,
            plast: None,
            praw: None,
            moved: Instant::now(),
            next: Instant::now(),
        }
        .moved_to(center_hz, span_hz)
    }

    /// The view at `center_hz`, `span_hz`: its bins, and its power levels
    /// from the last power line.
    fn moved_to(mut self, center_hz: f64, span_hz: f64) -> Self {
        self.move_to(center_hz, span_hz);
        self
    }

    fn move_to(&mut self, center_hz: f64, span_hz: f64) {
        self.center_hz = center_hz;
        self.span_hz = span_hz;
        self.view = view_bins(
            &self.caps,
            center_hz - span_hz / 2.0,
            center_hz + span_hz / 2.0,
        );
        self.psum = vec![0.0; self.view.len()];
        self.plines = 0;
        self.plast = self.praw.as_ref().map(|p| {
            self.view
                .iter()
                .map(|&k| db(p.get(k).copied().unwrap_or(0.0) as f64))
                .collect()
        });
        self.moved = Instant::now();
    }

    fn zoomed(&self) -> bool {
        self.view.len() <= ZOOM_BINS
    }

    fn fps(&self) -> f64 {
        self.subs.iter().map(|s| s.s.fps).fold(1.0, f64::max)
    }

    /// The FFTs for the finest display bins and fastest rate asked for,
    /// and the tap told what the view wants.
    fn fit(&mut self) {
        let fps = self.fps();
        if !self.zoomed() {
            self.zoom = None;
            self.tap.want(Vec::new(), fps);
            return;
        }
        let px = self.subs.iter().map(|s| s.s.bins).max().unwrap_or(1024);
        // FFT bins no wider than the display's
        let fs = self.caps.frame_rate;
        let n = ((fs * px as f64 / self.span_hz).ceil() as usize)
            .next_power_of_two()
            .clamp(64, 8192);
        if !self.zoom.as_ref().is_some_and(|z| z.n == n && z.fps == fps) {
            self.zoom = Some(Zoom {
                n,
                fps,
                engs: Vec::new(),
                map: None,
            });
        }
        let spec = WfSpec {
            fs,
            bins: n,
            budget: self.budget,
            fps,
            droop: Droop::None,
            fft_per_bin: 1,
        };
        let m = MARGIN as f64 * self.caps.bin_hz;
        let lo = self.center_hz - self.span_hz / 2.0;
        let hi = self.center_hz + self.span_hz / 2.0;
        let run = view_bins(&self.caps, lo - m, hi + m);
        self.zoom.as_mut().expect("set").keep(&run, spec);
        // the shown bins from the centre out, then the margin: what the
        // frames have no room for goes from the edges
        let mut want = self.view.clone();
        let c = self.center_hz;
        want.sort_by(|&a, &b| {
            let d = |k: usize| (self.caps.freq(k) - c).abs();
            d(a).total_cmp(&d(b))
        });
        want.extend(run.iter().filter(|b| !self.view.contains(b)));
        self.tap.want(want, fps);
    }

    fn commands(&mut self, now: Instant) -> bool {
        let mut changed = false;
        loop {
            match self.cmd.try_recv() {
                Ok(ViewCmd::Add(s)) => {
                    match self.subs.iter_mut().find(|x| x.s.session == s.session) {
                        Some(x) => {
                            let new_gen = x.s.new_gen || s.new_gen;
                            x.s = s;
                            x.s.new_gen = new_gen;
                        }
                        None => self.subs.push(Sub { s, next: now }),
                    }
                    changed = true;
                }
                Ok(ViewCmd::Remove(id)) => {
                    self.subs.retain(|x| x.s.session != id);
                    changed = true;
                }
                Ok(ViewCmd::Move { center_hz, span_hz }) => {
                    self.move_to(center_hz, span_hz);
                    changed = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
        if changed {
            if self.subs.is_empty() {
                self.tap.want(Vec::new(), 0.0);
            } else {
                self.fit();
            }
        }
        true
    }

    /// The tap's frames and power lines.
    fn input(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                TapMsg::Frames(f) => {
                    if let Some(z) = &mut self.zoom {
                        z.frames(&f);
                    }
                }
                TapMsg::Power(line) => {
                    for (s, &k) in self.psum.iter_mut().zip(&self.view) {
                        *s += line.get(k).copied().unwrap_or(0.0) as f64;
                    }
                    self.plines += 1;
                    self.praw = Some(line);
                }
            }
        }
    }

    /// The power lines since the last line, averaged (dB per view bin).
    fn power_line(&mut self) -> Option<(Vec<f32>, bool)> {
        if self.plines > 0 {
            let inv = 1.0 / self.plines as f64;
            let db = self.psum.iter().map(|&p| db(p * inv)).collect::<Vec<_>>();
            self.psum.fill(0.0);
            self.plines = 0;
            self.plast = Some(db.clone());
            return Some((db, true));
        }
        self.plast.clone().map(|db| (db, false))
    }

    /// This line, finely binned over the view's bins: levels, the first
    /// one's frequency, the spacing, whether anything new came. `None`
    /// while there is nothing yet, or while bins just added fill.
    fn line(&mut self, now: Instant) -> Option<(Vec<f32>, f64, f64, bool)> {
        let bin_hz = self.caps.bin_hz;
        let first = self.caps.freq(*self.view.first()?);
        if let Some(z) = &self.zoom
            && now < self.moved + SETTLE
            && z.engs
                .iter()
                .any(|e| self.view.contains(&e.bin) && !e.e.ready())
        {
            return None;
        }
        let power = self.power_line();
        let Some(z) = &mut self.zoom else {
            let (db, fresh) = power?;
            return Some((db, first, bin_hz, fresh));
        };
        let df = self.caps.frame_rate / z.n as f64;
        // the FFT bins within ±bin_hz/2 of the centre
        let keep = (bin_hz / df).round() as usize;
        let j0 = z.n / 2 - keep / 2;
        let mut db = Vec::with_capacity(keep * self.view.len());
        let mut fresh = false;
        let mut any = power.is_some();
        // the power line's level spread over the FFT bins (as a density:
        // a 30 kHz bin's noise is 10 log10(keep) dB above one FFT bin's)
        let spread = 10.0 * (keep as f32).log10();
        for e in &mut z.engs {
            // the margin's lines too: they stay current
            let l = e.e.line();
            if let Some(l) = &l
                && l.fresh
            {
                e.held = Some((l.db[j0..j0 + keep].to_vec(), now));
            }
            let Some(i) = self.view.iter().position(|&b| b == e.bin) else {
                continue;
            };
            any |= l.is_some() || e.held.is_some();
            match (&l, &e.held) {
                (Some(l), _) if l.fresh => {
                    db.extend_from_slice(&l.db[j0..j0 + keep]);
                    fresh = true;
                }
                // missing from the frames a while: its last spectrum
                (_, Some((h, t))) if now < *t + HOLD => db.extend_from_slice(h),
                // not in the frames: the power line's level, interpolated
                // towards the neighbours' between the bins' centres
                _ => match &power {
                    Some((p, _)) => {
                        let at = |k: usize| (p[k] - spread).max(WF_DB0);
                        let lo = at(i.saturating_sub(1));
                        let hi = at((i + 1).min(p.len() - 1));
                        let mid = at(i);
                        db.extend((0..keep).map(|m| {
                            let x = (m as f32 + 0.5) / keep as f32 - 0.5;
                            if x < 0.0 {
                                mid + (lo - mid) * -x
                            } else {
                                mid + (hi - mid) * x
                            }
                        }));
                    }
                    None => db.extend(std::iter::repeat_n(WF_DB0, keep)),
                },
            }
        }
        if !any {
            return None; // nothing yet
        }
        fresh |= power.is_some_and(|p| p.1);
        let f0 = first + (j0 as f64 - z.n as f64 / 2.0) * df;
        Some((db, f0, df, fresh))
    }

    /// The view's part of the line to each subscriber due.
    fn send(&mut self, db: &[f32], f0: f64, df: f64, fresh: bool, now: Instant, p: Duration) {
        let lo = self.center_hz - self.span_hz / 2.0;
        let hi = self.center_hz + self.span_hz / 2.0;
        let k0 = ((lo - f0) / df).ceil().max(0.0) as usize;
        let k1 = ((((hi - f0) / df).floor() + 1.0).max(0.0) as usize).min(db.len());
        if k0 >= k1 {
            return;
        }
        let mid = &db[k0..k1];
        let span = self.span_hz;
        self.subs.retain_mut(|x| {
            if now < x.next {
                return true;
            }
            x.next = (x.next + p.max(period(x.s.fps))).max(now);
            let g = ((span / x.s.bins.max(1) as f64 / df).round() as usize).max(1);
            let out = rebin(mid, g);
            let first = f0 + k0 as f64 * df + (g as f64 - 1.0) / 2.0 * df;
            let new_gen = std::mem::take(&mut x.s.new_gen);
            x.s.sink.send(new_gen, fresh, first, df * g as f64, &out)
        });
    }
}

impl Job for BinViewJob {
    fn tick(&mut self, now: Instant, ctx: Ctx) -> bool {
        if !self.commands(now) {
            return false;
        }
        self.input();
        if now < self.next || self.subs.is_empty() {
            return true;
        }
        let (budget, p) = shed(self.budget, self.fps(), ctx.shed);
        self.next = (self.next + p).max(now);
        if let Some(z) = &mut self.zoom {
            for e in &mut z.engs {
                e.e.set_budget(budget);
            }
        }
        if let Some((db, f0, df, fresh)) = self.line(now) {
            self.send(&db, f0, df, fresh, now, p);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::{Out, WfSink};
    use std::f64::consts::PI;
    use std::sync::atomic::AtomicU64;
    use web888_radio::{Iq32, RX_SINE_FULL_SCALE};

    /// The channelizer bitstream's bins.
    const CAPS: BinCaps = BinCaps {
        bin_hz: 30e3,
        frame_rate: 60e3,
        bins: 2048,
        first_hz: 0.0,
        wraps: false,
        max_selected: 64,
    };

    /// Frames `f0..f0 + n` of bins `sel`: each bin sees the tones within
    /// ±30 kHz of its centre (amplitude 1: a full-scale sine).
    fn frames(sel: &[usize], f0: u64, n: usize, tones: &[(f64, f64)]) -> Arc<Vec<BinFrames>> {
        let bins: Arc<[usize]> = Arc::from(sel.to_vec());
        let mut out = Vec::new();
        for frame in f0..f0 + n as u64 {
            let t = frame as f64 / CAPS.frame_rate;
            let iq = sel.iter().map(|&k| {
                let (mut i, mut q) = (0.0, 0.0);
                for &(hz, a) in tones {
                    let off = hz - CAPS.freq(k);
                    if off.abs() < 30e3 {
                        let p = 2.0 * PI * off * t;
                        i += a * p.cos();
                        q += a * p.sin();
                    }
                }
                let s = RX_SINE_FULL_SCALE;
                Iq32 {
                    i: (i * s) as i32,
                    q: (q * s) as i32,
                }
            });
            BinFrames::append(&mut out, frame, 1, &bins, iq);
        }
        Arc::new(out)
    }

    struct Line {
        first_hz: f64,
        bin_hz: f64,
        db: Vec<f32>,
    }

    impl Line {
        fn at(&self, hz: f64) -> f32 {
            self.db[((hz - self.first_hz) / self.bin_hz).round() as usize]
        }
    }

    fn decode(f: &[u8]) -> Line {
        let f64_at = |o: usize| f64::from_le_bytes(f[o..o + 8].try_into().unwrap());
        let f32_at = |o: usize| f32::from_le_bytes(f[o..o + 4].try_into().unwrap());
        let n = u16::from_le_bytes([f[2], f[3]]) as usize;
        let (db0, step) = (f32_at(24), f32_at(28));
        Line {
            first_hz: f64_at(8),
            bin_hz: f64_at(16),
            db: f[40..40 + n]
                .iter()
                .map(|&v| db0 + v as f32 * step)
                .collect(),
        }
    }

    fn newest(rx: &mut tokio::sync::mpsc::Receiver<Vec<u8>>) -> Line {
        let mut last = None;
        while let Ok(f) = rx.try_recv() {
            last = Some(f);
        }
        decode(&last.expect("a line"))
    }

    /// A job for the view and a session watching it at 1024 bins.
    fn job(
        tap: &BinTap,
        center: f64,
        span: f64,
    ) -> (
        BinViewJob,
        tokio::sync::mpsc::Receiver<Vec<u8>>,
        crossbeam_channel::Sender<ViewCmd>,
    ) {
        let (ctx, crx) = crossbeam_channel::unbounded();
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let sink = WfSink::new(Out {
            tx,
            drops: Arc::new(AtomicU64::new(0)),
        });
        ctx.send(ViewCmd::Add(ViewSub {
            session: 1,
            bins: 1024,
            fps: 10.0,
            sink,
            new_gen: true,
        }))
        .unwrap();
        (BinViewJob::new(center, span, 4, tap, crx), rx, ctx)
    }

    #[test]
    fn a_zoomed_view_tiles_the_bins_without_a_seam() {
        let tap = BinTap::new(CAPS);
        // 480 kHz around 7.074 MHz: bins 228..=244
        let (mut j, mut rx, _ctx) = job(&tap, 7.074e6, 480e3);
        let t0 = Instant::now();
        assert!(j.tick(t0, Ctx::default()));
        assert!(tap.wants_frames());
        assert_eq!(j.view.len(), 17);
        assert_eq!(j.view[0], 228);
        // a tone mid-bin and one on the edge of bins 236 and 237
        let tones = [(7.074e6, 0.1), (7.095e6, 0.1)];
        tap.send_frames(frames(&j.view, 0, 6000, &tones));
        assert!(j.tick(t0 + Duration::from_millis(200), Ctx::default()));
        let l = newest(&mut rx);
        // 1024 bins of 468.75 Hz from the view's low edge
        assert_eq!(l.bin_hz, 468.75);
        assert!((l.db.len() as i64 - 1024).abs() <= 1, "{}", l.db.len());
        assert!((l.first_hz - (7.074e6 - 240e3)).abs() <= l.bin_hz);
        for hz in [7.074e6, 7.095e6] {
            let p = (-2..=2)
                .map(|k| l.at(hz + k as f64 * l.bin_hz))
                .fold(-200f32, f32::max);
            assert!((p + 20.0).abs() < 1.0, "{hz}: {p} dB");
        }
        // the floor elsewhere is the quantiser's
        assert!(l.at(7.0e6) < -120.0, "{}", l.at(7.0e6));
    }

    #[test]
    fn bins_missing_from_the_frames_show_the_power_line() {
        let tap = BinTap::new(CAPS);
        let (mut j, mut rx, _ctx) = job(&tap, 7.074e6, 480e3);
        let t0 = Instant::now();
        j.tick(t0, Ctx::default());
        let sel: Vec<usize> = j.view.iter().copied().filter(|&k| k != 238).collect();
        tap.send_frames(frames(&sel, 0, 6000, &[]));
        let mut p = vec![0f32; 2048];
        p[238] = 1e-3;
        tap.send_power(Arc::new(p));
        // held while the view settles: the bin may yet come
        j.tick(t0 + Duration::from_millis(100), Ctx::default());
        assert!(rx.try_recv().is_err());
        j.tick(t0 + SETTLE + Duration::from_millis(50), Ctx::default());
        let l = newest(&mut rx);
        // bin 238 (7.14 MHz) at the power line's -30 dB spread over its 30
        // kHz (64 FFT bins of 468.75 Hz, each 18.1 dB below) at its centre,
        // sloping to the neighbours' levels (none: the floor) at the edges
        let d = l.at(7.14e6);
        assert!((d + 48.06).abs() < 1.0, "centre: {d}");
        for off in [-14e3, 14e3] {
            let e = l.at(7.14e6 + off);
            assert!(e < d - 50.0, "{off}: {e}");
        }
        assert!(l.at(7.1e6) < -120.0);
    }

    #[test]
    fn a_dragged_view_keeps_its_ffts() {
        let tap = BinTap::new(CAPS);
        // 60 kHz around 7.074 MHz: bins 235..=237 shown, 234 and 238 run
        let (mut j, mut rx, ctx) = job(&tap, 7.074e6, 60e3);
        let t0 = Instant::now();
        j.tick(t0, Ctx::default());
        assert_eq!(j.view, vec![235, 236, 237]);
        let tones = [(7.074e6, 0.1), (7.11e6, 0.1)];
        let run = [234, 235, 236, 237, 238];
        tap.send_frames(frames(&run, 0, 6000, &tones));
        j.tick(t0 + Duration::from_millis(100), Ctx::default());
        newest(&mut rx);
        let bins = |j: &BinViewJob| {
            j.zoom
                .as_ref()
                .unwrap()
                .engs
                .iter()
                .map(|e| e.bin)
                .collect::<Vec<_>>()
        };
        assert_eq!(bins(&j), vec![234, 235, 236, 237, 238]);
        // 30 kHz up: bin 238 is shown at once, from the margin's FFTs
        ctx.send(ViewCmd::Move {
            center_hz: 7.104e6,
            span_hz: 60e3,
        })
        .unwrap();
        tap.send_frames(frames(&run, 6000, 6000, &tones));
        j.tick(t0 + Duration::from_millis(200), Ctx::default());
        assert_eq!(j.view, vec![236, 237, 238]);
        let l = newest(&mut rx);
        assert!((l.at(7.11e6) + 20.0).abs() < 1.0, "{}", l.at(7.11e6));
        assert_eq!(bins(&j), vec![235, 236, 237, 238, 239]);
    }

    #[test]
    fn a_bin_gone_from_the_frames_shows_its_last_spectrum_a_while() {
        let tap = BinTap::new(CAPS);
        let (mut j, mut rx, _ctx) = job(&tap, 7.074e6, 480e3);
        let t0 = Instant::now();
        j.tick(t0, Ctx::default());
        let tones = [(7.14e6, 0.1)];
        tap.send_frames(frames(&j.view, 0, 6000, &tones));
        j.tick(t0 + SETTLE, Ctx::default());
        newest(&mut rx);
        // bin 238 drops out (no room, or the queue overflowed)
        let sel: Vec<usize> = j.view.iter().copied().filter(|&k| k != 238).collect();
        tap.send_frames(frames(&sel, 6000, 6000, &tones));
        j.tick(t0 + SETTLE + Duration::from_millis(500), Ctx::default());
        let l = newest(&mut rx);
        let p = (-2..=2)
            .map(|k| l.at(7.14e6 + k as f64 * l.bin_hz))
            .fold(-200f32, f32::max);
        assert!((p + 20.0).abs() < 1.0, "held: {p} dB");
        // later: the power line's level (none here: the floor)
        tap.send_frames(frames(&sel, 12000, 6000, &tones));
        j.tick(
            t0 + SETTLE + HOLD + Duration::from_millis(500),
            Ctx::default(),
        );
        let l = newest(&mut rx);
        assert!(l.at(7.14e6) < -120.0, "{}", l.at(7.14e6));
    }

    #[test]
    fn a_wide_view_draws_the_power_line() {
        let tap = BinTap::new(CAPS);
        // 7.68 MHz around 7.68 MHz: 257 bins, too many to zoom
        let (mut j, mut rx, _ctx) = job(&tap, 7.68e6, 7.68e6);
        let t0 = Instant::now();
        j.tick(t0, Ctx::default());
        assert!(!tap.wants_frames());
        for v in [1e-2, 1e-4] {
            let mut p = vec![1e-12f32; 2048];
            p[236] = v;
            tap.send_power(Arc::new(p));
        }
        j.tick(t0 + Duration::from_millis(200), Ctx::default());
        let l = newest(&mut rx);
        // the bins as they are (7.5 kHz display bins are finer)
        assert_eq!(l.bin_hz, 30e3);
        assert_eq!(l.first_hz, 3.84e6);
        // two lines averaged: (1e-2 + 1e-4) / 2
        assert!((l.at(7.08e6) + 22.97).abs() < 0.5, "{}", l.at(7.08e6));
        assert!(l.at(7.11e6) < -110.0);
    }

    #[test]
    fn view_bins_by_frequency() {
        let iq = BinCaps {
            first_hz: 8.6e6,
            bins: 256,
            wraps: true,
            ..CAPS
        };
        // bins below the centre wrap: 255 is 8.57 MHz
        assert_eq!(view_bins(&iq, 8.55e6, 8.65e6), vec![254, 255, 0, 1, 2]);
    }
}
