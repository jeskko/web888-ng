// SPDX-License-Identifier: GPL-3.0-or-later
//! Waterfall views from a channelizer's bins (`doc/m6-channelizer.md` §4),
//! for radios without WF slots in bins mode:
//!
//! * a **zoomed** view (at most [`ZOOM_BINS`] bins: 7.68 MHz; on a zoom
//!   engine's jobs [`ENGINE_BINS`]: 30.72 MHz) takes the
//!   bins it covers through a [`BinTap`] and puts each through its own
//!   FFT; the middle 30 kHz of each (the bins are 60 kS/s, 30 kHz apart,
//!   flat well past ±15 kHz) side by side tile the view without a seam;
//! * a **wide** view draws the power line: 30 kHz per bin, merged into
//!   the view's display bins.
//!
//! A zoomed view reads its bins in a burst after each line, a line's
//! FFTs' worth, in groups as the frames have room after the RX channels'
//! bins; a bin left out shows its last spectrum, then the power line's
//! level. Under load (the pool shedding) only views up to [`LIGHT_BINS`]
//! are zoomed.
//!
//! With a zoom engine in the source (`hw/docs/channelizer.md` §3.9) a
//! zoomed view of FFTs up to the engine's length runs none of its own: it
//! hands the tap jobs, a spectrum's (one job, or more past the engine's
//! limits) at a time, as many a line as its share of the engine allows
//! (`web888_dsp::zoom::schedule`), spread over the line, and sums their
//! results into its line. A job's samples are 2-34 ms old when its result
//! comes, so a moved view just asks for its new bins.

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
use web888_dsp::zoom::{Demand, Part, ZoomAcc, plan, runs, shift_for};
use web888_radio::bins::{BinCaps, BinFrames, ZoomCaps, ZoomJob, ZoomResult};

/// Most bins a zoomed view shows (a 7.68 MHz span at 30 kHz: 257); wider
/// views draw the power line. A bin read all along costs the A9 about 1 %
/// of a core (memory-bound), so views read theirs in bursts (a group at
/// least one hw read, 300 frames: about 0.1 % a bin); a view's FFTs cost
/// about 10 % whatever its span. Bins from the view's centre out.
pub const ZOOM_BINS: usize = 257;

/// Most bins a view on the zoom engine's jobs shows (a 30.72 MHz span:
/// 1025 bins of 8-point FFTs, five jobs a spectrum), past [`ZOOM_BINS`]
/// only for a display finer than the bins (15.36 MHz; 30.72 MHz on 2048
/// points). On the unit such a view costs the pool 3 to 5 % of a core
/// and its noise floor moves 2 to 3 dB from line to line, the power
/// line's 0.3 dB (`doc/m6-channelizer.md` §6).
pub const ENGINE_BINS: usize = 1025;

/// Most FFT points a line of a view on the engine's jobs sums (its
/// spectra a line times its bins' points; one spectrum at least): what
/// the view costs the pool. A 480 kHz view's six spectra are under it; a
/// 15.36 MHz view gets four, a 30.72 MHz one two.
const LINE_POINTS: usize = 8400;

/// Most bins a zoomed view shows while the pool sheds load (960 kHz).
pub const LIGHT_BINS: usize = 33;

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

/// How long a job-fed view's raised shift (a result saturated) lasts.
const SAT_HOLD: Duration = Duration::from_secs(5);

/// A zoomed view on the zoom engine's jobs.
struct Jobs {
    logn: u8,
    /// The view's bins these jobs are for.
    bins: Vec<usize>,
    parts: Vec<Part>,
    acc: ZoomAcc,
    /// Tags outstanding and their parts.
    tags: Vec<(u8, usize)>,
    /// Queued jobs of an older plan to drop with the next submit.
    stale: bool,
    /// Spectra this line: asked for, to ask for, from when.
    sent: usize,
    a: usize,
    start: Instant,
    period: Duration,
    /// Each view bin's last levels (n/2 each) and when they came.
    held: Vec<f32>,
    held_at: Vec<Option<Instant>>,
    /// Shift added after a saturated result, and when.
    extra: u8,
    sat: Instant,
}

impl Jobs {
    fn new(caps: &BinCaps, zc: &ZoomCaps, view: &[usize], logn: u8, now: Instant) -> Self {
        Jobs {
            logn,
            bins: view.to_vec(),
            parts: plan(caps, zc, view, logn),
            acc: ZoomAcc::new(view.len(), 1 << logn),
            tags: Vec::new(),
            stale: true,
            sent: 0,
            a: 1,
            start: now,
            period: Duration::from_millis(50),
            held: vec![0.0; view.len() << (logn - 1)],
            held_at: vec![None; view.len()],
            extra: 0,
            sat: now,
        }
    }

    /// A result: summed if it is one of this plan's.
    fn result(&mut self, r: ZoomResult, now: Instant) {
        let Some(k) = self.tags.iter().position(|t| t.0 == r.tag) else {
            return;
        };
        let (_, p) = self.tags.swap_remove(k);
        let part = self.parts[p];
        if r.power.len() == part.bins << (self.logn - 1) {
            self.acc.add(part.at, &r.power);
        }
        if r.saturated {
            self.extra = (self.extra + 1).min(4);
            self.sat = now;
        }
    }

    /// One spectrum's jobs to the tap, each shifted for its bins'
    /// strongest power line level.
    fn submit(&mut self, tap: &TapHandle, power: Option<&[f32]>, now: Instant) {
        if self.extra > 0 && now > self.sat + SAT_HOLD {
            self.extra -= 1;
            self.sat = now;
        }
        let jobs: Vec<ZoomJob> = self
            .parts
            .iter()
            .map(|p| {
                let peak = power.map_or(0.0, |l| {
                    self.bins[p.at..p.at + p.bins]
                        .iter()
                        .map(|&k| l.get(k).copied().unwrap_or(0.0))
                        .fold(0.0, f32::max)
                });
                ZoomJob {
                    tag: 0,
                    shift: (shift_for(peak) + self.extra).min(7),
                    logn: self.logn,
                    bins: p.bins as u16,
                    k0: p.k0 as u16,
                }
            })
            .collect();
        let tags = tap.submit_zoom(&jobs, std::mem::take(&mut self.stale));
        self.tags.extend(tags.into_iter().zip(0..));
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
    /// The bins a zoomed view reads (centre out, then the margin), and
    /// its FFTs a line (load shedding may lower it).
    want: Vec<usize>,
    ffts: usize,
    /// The pool sheds load: only light views are zoomed.
    loaded: bool,
    /// The source's zoom engine, and the view on its jobs.
    engine: Option<ZoomCaps>,
    jobs: Option<Jobs>,
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
            want: Vec::new(),
            ffts: budget.max(1),
            loaded: false,
            engine: tap.zoom_caps(),
            jobs: None,
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
        // on the engine's jobs a view costs the pool little
        let most = match self.engine {
            Some(zc) if self.fft() <= 1 << zc.max_logn => ENGINE_BINS,
            _ if self.loaded => LIGHT_BINS,
            _ => ZOOM_BINS,
        };
        // past ZOOM_BINS only for a display finer than the bins: the
        // power line is as sharp otherwise, and far smoother
        let px = self.subs.iter().map(|s| s.s.bins).max().unwrap_or(1024);
        let finer = self.span_hz / (px as f64) < self.caps.bin_hz;
        self.view.len() <= most && (self.view.len() <= ZOOM_BINS || finer)
    }

    /// The FFT length for the finest display bins asked for: FFT bins no
    /// wider than the display's.
    fn fft(&self) -> usize {
        let px = self.subs.iter().map(|s| s.s.bins).max().unwrap_or(1024);
        ((self.caps.frame_rate * px as f64 / self.span_hz).ceil() as usize)
            .next_power_of_two()
            .clamp(8, 8192)
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
            self.jobs = None;
            self.tap.zoom_demand(None);
            self.want.clear();
            self.tap.want(Vec::new(), fps);
            self.tap.burst(Vec::new(), 0, true);
            return;
        }
        let fs = self.caps.frame_rate;
        let n = self.fft();
        if let Some(zc) = self.engine
            && n <= 1 << zc.max_logn
        {
            self.zoom = None;
            self.want.clear();
            self.tap.want(Vec::new(), fps);
            self.tap.burst(Vec::new(), 0, true);
            let logn = (n.trailing_zeros() as u8).max(zc.min_logn);
            let points = self.view.len() << (logn - 1);
            self.tap.zoom_demand(Some(Demand {
                runs: runs(&self.caps, &self.view),
                logn,
                fps,
                max_a: self.budget.min(LINE_POINTS / points).max(1),
            }));
            self.plan_jobs(Instant::now());
            return;
        }
        self.jobs = None;
        self.tap.zoom_demand(None);
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
        self.want = want;
        // the power lines all along, the bins in bursts
        self.tap.want(Vec::new(), fps);
        self.burst(true);
    }

    /// Jobs for the view's bins at the granted FFT length; a plan for
    /// the same keeps its line, one for other bins keeps the levels of
    /// the bins it still shows.
    fn plan_jobs(&mut self, now: Instant) {
        let (Some(zc), Some(g)) = (self.engine, self.tap.zoom_grant()) else {
            self.jobs = None;
            return;
        };
        if let Some(j) = &self.jobs
            && j.logn == g.logn
            && j.bins == self.view
        {
            return;
        }
        let mut new = Jobs::new(&self.caps, &zc, &self.view, g.logn, now);
        if let Some(old) = self.jobs.take() {
            new.extra = old.extra;
            new.sat = old.sat;
            new.start = old.start;
            new.period = old.period;
            new.a = old.a;
            if old.logn == g.logn {
                let keep = 1usize << (g.logn - 1);
                // both by frequency: the old view's bins from where the
                // new one's first is
                let off = old.bins.iter().position(|x| Some(x) == self.view.first());
                for (k, b) in self.view.iter().enumerate() {
                    let i = match off {
                        Some(o) if old.bins.get(o + k) == Some(b) => o + k,
                        _ => match old.bins.iter().position(|x| x == b) {
                            Some(i) => i,
                            None => continue,
                        },
                    };
                    new.held_at[k] = old.held_at[i];
                    new.held[k * keep..(k + 1) * keep]
                        .copy_from_slice(&old.held[i * keep..(i + 1) * keep]);
                }
            }
        }
        self.jobs = Some(new);
    }

    /// The bins for the next line's FFTs: `ffts` of `n` at half overlap
    /// (`restart`: also if the last burst isn't done; the view changed).
    fn burst(&self, restart: bool) {
        if let Some(z) = &self.zoom {
            self.tap
                .burst(self.want.clone(), z.n * (self.ffts + 1) / 2, restart);
        }
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
                self.tap.burst(Vec::new(), 0, true);
                self.tap.zoom_demand(None);
                self.jobs = None;
            } else {
                self.fit();
            }
        }
        true
    }

    /// The tap's frames and power lines.
    fn input(&mut self, now: Instant) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                TapMsg::Zoom(r) => {
                    if let Some(j) = &mut self.jobs {
                        j.result(r, now);
                    }
                }
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
        if self.jobs.is_some() {
            return self.job_line(power, first, now);
        }
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
                _ => power_fill(&mut db, power.as_ref().map(|p| &p.0[..]), i, keep),
            }
        }
        if !any {
            return None; // nothing yet
        }
        fresh |= power.is_some_and(|p| p.1);
        let f0 = first + (j0 as f64 - z.n as f64 / 2.0) * df;
        Some((db, f0, df, fresh))
    }

    /// [`line`](Self::line) from the jobs' results: the spectra summed
    /// since the last line, a bin without one its last levels a while,
    /// then the power line's.
    fn job_line(
        &mut self,
        power: Option<(Vec<f32>, bool)>,
        first: f64,
        now: Instant,
    ) -> Option<(Vec<f32>, f64, f64, bool)> {
        let j = self.jobs.as_mut()?;
        let n = 1usize << j.logn;
        let keep = n / 2;
        let df = self.caps.frame_rate / n as f64;
        let mut db = Vec::with_capacity(keep * self.view.len());
        let mut fresh = false;
        let mut any = power.is_some();
        for i in 0..self.view.len() {
            let at = db.len();
            let h = &mut j.held[i * keep..(i + 1) * keep];
            if j.acc.bin_db(i, &mut db) {
                h.copy_from_slice(&db[at..]);
                j.held_at[i] = Some(now);
                fresh = true;
                any = true;
                continue;
            }
            match j.held_at[i] {
                Some(t) if now < t + HOLD => {
                    db.extend_from_slice(h);
                    any = true;
                }
                _ => power_fill(&mut db, power.as_ref().map(|p| &p.0[..]), i, keep),
            }
        }
        j.acc.clear();
        if !any {
            return None;
        }
        fresh |= power.is_some_and(|p| p.1);
        let f0 = first - (n / 4) as f64 * df;
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
        self.input(now);
        if self.subs.is_empty() {
            return true;
        }
        if now < self.next {
            self.submit_jobs(now);
            return true;
        }
        let (budget, mut p) = shed(self.budget, self.fps(), ctx.shed);
        if self.jobs.is_some() {
            // the engine's share: the line's spectra and the rate
            self.plan_jobs(now);
            if let Some(g) = self.tap.zoom_grant() {
                p = p.max(period(g.fps));
                if let Some(j) = &mut self.jobs {
                    j.a = g.a.min(budget).max(1);
                }
            }
        }
        self.next = (self.next + p).max(now);
        if self.loaded != (ctx.shed > 0) {
            self.loaded = ctx.shed > 0;
            self.fit();
        }
        if let Some(z) = &mut self.zoom {
            for e in &mut z.engs {
                e.e.set_budget(budget);
            }
        }
        self.ffts = budget;
        if let Some((db, f0, df, fresh)) = self.line(now) {
            self.send(&db, f0, df, fresh, now, p);
        }
        // the next line's samples (a wide view's last burst may still
        // be going: it goes on)
        if let Some(j) = &mut self.jobs {
            j.start = now;
            j.period = p;
            j.sent = 0;
        } else {
            self.burst(false);
        }
        self.submit_jobs(now);
        true
    }
}

impl BinViewJob {
    /// A job-fed view's spectra due by now: the line's `a` spread over
    /// its period.
    fn submit_jobs(&mut self, now: Instant) {
        let Some(j) = &mut self.jobs else { return };
        while j.sent < j.a && now >= j.start + j.period * j.sent as u32 / j.a as u32 {
            j.submit(&self.tap, self.praw.as_deref().map(|v| &v[..]), now);
            j.sent += 1;
        }
    }
}

/// A bin without a spectrum (the `i`th of the view's): the power line's
/// level spread over its `keep` points (as a density), interpolated
/// towards the neighbours' between the bins' centres; the floor without
/// a power line.
fn power_fill(db: &mut Vec<f32>, power: Option<&[f32]>, i: usize, keep: usize) {
    let Some(p) = power else {
        db.extend(std::iter::repeat_n(WF_DB0, keep));
        return;
    };
    // a 30 kHz bin's noise is 10 log10(keep) dB above one FFT bin's
    let spread = 10.0 * (keep as f32).log10();
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
        // 15.36 MHz from 0: 513 bins, too many to zoom
        let (mut j, mut rx, _ctx) = job(&tap, 7.68e6, 15.36e6);
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
        // the bins as they are (15 kHz display bins are finer)
        assert_eq!(l.bin_hz, 30e3);
        assert_eq!(l.first_hz, 0.0);
        // two lines averaged: (1e-2 + 1e-4) / 2
        assert!((l.at(7.08e6) + 22.97).abs() < 0.5, "{}", l.at(7.08e6));
        assert!(l.at(7.11e6) < -110.0);
    }

    #[test]
    fn a_vhf_view_starts_at_the_zone_edge() {
        // the VHF channelizer: 24 kHz bins from 98.304 MHz (undersampled)
        let vhf = BinCaps {
            bin_hz: 24e3,
            frame_rate: 48e3,
            first_hz: 98.304e6,
            ..CAPS
        };
        let tap = BinTap::new(vhf);
        // 12.288 MHz (512 bins): too many to zoom, the power line
        let (mut j, mut rx, _ctx) = job(&tap, 125e6, 12.288e6);
        let t0 = Instant::now();
        j.tick(t0, Ctx::default());
        assert!(!tap.wants_frames());
        // 125 MHz is in bin 1112 (124.992 MHz)
        let mut p = vec![1e-12f32; 2048];
        p[1112] = 1e-2;
        tap.send_power(Arc::new(p));
        j.tick(t0 + Duration::from_millis(200), Ctx::default());
        let l = newest(&mut rx);
        // on the bins' grid, counted from the zone's edge
        assert_eq!(l.bin_hz, 24e3);
        let k = (l.first_hz - 98.304e6) / 24e3;
        assert!(
            (k - k.round()).abs() < 1e-6 && l.first_hz > 118e6,
            "{}",
            l.first_hz
        );
        assert!((l.at(125e6) + 20.0).abs() < 0.5, "{}", l.at(125e6));
        assert!(l.at(125.1e6) < -110.0);
    }

    #[test]
    fn a_7_68_mhz_view_is_zoomed_unless_the_pool_sheds() {
        let tap = BinTap::new(CAPS);
        let (mut j, mut rx, _ctx) = job(&tap, 7.68e6, 7.68e6);
        let t0 = Instant::now();
        j.tick(t0, Ctx::default());
        assert!(tap.wants_frames());
        let z = j.zoom.as_ref().expect("zoomed");
        // 8-point FFTs: 7.5 kHz, the display's bins
        assert_eq!((z.n, z.engs.len()), (8, 259));
        let tones = [(7.08e6, 0.1)];
        tap.send_frames(frames(&j.view, 0, 3000, &tones));
        j.tick(t0 + SETTLE, Ctx::default());
        let l = newest(&mut rx);
        assert_eq!(l.bin_hz, 7.5e3);
        let p = (-1..=1)
            .map(|k| l.at(7.08e6 + k as f64 * l.bin_hz))
            .fold(-200f32, f32::max);
        assert!((p + 20.0).abs() < 1.5, "{p} dB");
        // under load: the power line
        let busy = Ctx { shed: 1 };
        j.tick(t0 + SETTLE + Duration::from_millis(100), busy);
        assert!(j.zoom.is_none());
    }

    /// The engine's side of a tap: the iq backend's engine on frames of
    /// every bin, `tones` in them as in [`frames`].
    struct Engine {
        e: web888_iq::zoom::ZoomEngine,
        frame: u64,
        jobs: Vec<ZoomJob>,
        u: Vec<web888_dsp::C32>,
    }

    impl Engine {
        fn new() -> Self {
            Engine {
                e: web888_iq::zoom::ZoomEngine::new(ZoomCaps::ENGINE, CAPS.bins),
                frame: 0,
                jobs: Vec::new(),
                u: vec![web888_dsp::C32::default(); CAPS.bins],
            }
        }

        /// 5 ms: the queued jobs to the engine, 300 frames, the results
        /// back.
        fn run(&mut self, tap: &BinTap, tones: &[(f64, f64)]) {
            tap.zoom_submit(|j| {
                let k = self.e.submit(j);
                self.jobs.extend_from_slice(&j[..k]);
                Ok(k)
            })
            .unwrap();
            for _ in 0..300 {
                let t = self.frame as f64 / CAPS.frame_rate;
                self.u.fill(web888_dsp::C32::default());
                for &(hz, a) in tones {
                    let (k, _) = CAPS.bin_of(hz).unwrap();
                    for k in k - 1..=k + 1 {
                        let off = hz - CAPS.freq(k);
                        if off.abs() < 30e3 {
                            let p = (2.0 * PI * off * t).rem_euclid(2.0 * PI);
                            self.u[k] += web888_dsp::C32::from_polar(a as f32, p as f32);
                        }
                    }
                }
                self.e.frame(self.frame, &self.u);
                self.frame += 1;
            }
            let mut out = Vec::new();
            self.e.read(&mut out);
            tap.zoom_deliver(&mut out);
        }
    }

    /// Tick the view and run the engine for `ms` from `t0` in 5 ms steps;
    /// returns the time reached.
    fn run_jobs(
        j: &mut BinViewJob,
        tap: &BinTap,
        e: &mut Engine,
        t0: Instant,
        ms: u64,
        tones: &[(f64, f64)],
    ) -> Instant {
        let mut t = t0;
        for _ in 0..ms / 5 {
            j.tick(t, Ctx::default());
            e.run(tap, tones);
            t += Duration::from_millis(5);
        }
        t
    }

    fn peak(l: &Line, hz: f64) -> f32 {
        (-2..=2)
            .map(|k| l.at(hz + k as f64 * l.bin_hz))
            .fold(-200f32, f32::max)
    }

    #[test]
    fn a_view_on_the_engines_jobs_runs_no_ffts() {
        let tap = BinTap::with_zoom(CAPS, Some(ZoomCaps::ENGINE));
        let (mut j, mut rx, _ctx) = job(&tap, 7.074e6, 480e3);
        let mut e = Engine::new();
        let tones = [(7.074e6, 0.1), (7.095e6, 0.1)];
        let t = run_jobs(&mut j, &tap, &mut e, Instant::now(), 400, &tones);
        j.tick(t, Ctx::default());
        assert!(!tap.wants_frames());
        assert!(j.zoom.is_none());
        // one job a spectrum: the view's 17 bins of 128 points
        let job = e.jobs[0];
        assert_eq!((job.k0, job.bins, job.logn, job.shift), (228, 17, 7, 3));
        let l = newest(&mut rx);
        // as the view's own FFTs: 1024 bins of 468.75 Hz from its low edge
        assert_eq!(l.bin_hz, 468.75);
        assert!((l.db.len() as i64 - 1024).abs() <= 1, "{}", l.db.len());
        assert!((l.first_hz - (7.074e6 - 240e3)).abs() <= l.bin_hz);
        for hz in [7.074e6, 7.095e6] {
            let p = peak(&l, hz);
            assert!((p + 20.0).abs() < 1.0, "{hz}: {p} dB");
        }
        assert!(l.at(7.0e6) < -120.0, "{}", l.at(7.0e6));
    }

    #[test]
    fn a_15_36_mhz_view_on_the_engines_jobs() {
        let tap = BinTap::with_zoom(CAPS, Some(ZoomCaps::ENGINE));
        let (mut j, mut rx, _ctx) = job(&tap, 14.0e6, 15.36e6);
        let mut e = Engine::new();
        let tones = [(7.074e6, 0.1), (14.074e6, 0.1), (21.3e6, 0.1)];
        let t = run_jobs(&mut j, &tap, &mut e, Instant::now(), 400, &tones);
        j.tick(t, Ctx::default());
        assert!(j.zoom.is_none() && j.jobs.is_some());
        // 513 bins of 8 points in three jobs a spectrum
        assert_eq!(j.view.len(), 513);
        let b: Vec<_> = e.jobs[..3].iter().map(|j| (j.bins, j.logn)).collect();
        assert_eq!(b, [(171, 3); 3]);
        assert_eq!(e.jobs[1].k0, e.jobs[0].k0 + 171);
        let l = newest(&mut rx);
        // 15 kHz a display bin: two FFT points each
        assert_eq!(l.bin_hz, 15e3);
        assert!((l.db.len() as i64 - 1024).abs() <= 1, "{}", l.db.len());
        for (hz, _) in tones {
            let p = peak(&l, hz);
            assert!((p + 20.0).abs() < 3.0, "{hz}: {p} dB");
        }
        assert!(l.at(10.0e6) < -110.0, "{}", l.at(10.0e6));
        // 30.72 MHz on 1024 points (30 kHz each), the whole band: the
        // power line
        for span in [30.72e6, 61.44e6] {
            let (mut j, _rx, _ctx) = job(&tap, 30.72e6, span);
            j.tick(t, Ctx::default());
            assert!(j.jobs.is_none() && j.zoom.is_none(), "{span}");
        }
    }

    #[test]
    fn spectra_a_line_from_the_engines_share() {
        let tap = BinTap::with_zoom(CAPS, Some(ZoomCaps::ENGINE));
        // budget 4 at 10 lines/s: the engine has room for all 4
        let (mut j, _rx, _ctx) = job(&tap, 7.074e6, 480e3);
        let mut e = Engine::new();
        let t = run_jobs(&mut j, &tap, &mut e, Instant::now(), 1000, &[]);
        let n = e.jobs.len();
        assert!((38..=42).contains(&n), "{n} jobs in a second");
        // a bin above -6 dBFS on the power line: shift 4 for its job
        let mut p = vec![0f32; 2048];
        p[236] = 0.5;
        tap.send_power(Arc::new(p));
        run_jobs(&mut j, &tap, &mut e, t, 200, &[]);
        assert_eq!(e.jobs.last().unwrap().shift, 4);
    }

    #[test]
    fn a_moved_view_on_jobs_shows_its_new_bins_at_once() {
        let tap = BinTap::with_zoom(CAPS, Some(ZoomCaps::ENGINE));
        let (mut j, mut rx, ctx) = job(&tap, 7.074e6, 480e3);
        let mut e = Engine::new();
        let tones = [(7.074e6, 0.1), (7.4e6, 0.1)];
        let t = run_jobs(&mut j, &tap, &mut e, Instant::now(), 300, &tones);
        newest(&mut rx);
        ctx.send(ViewCmd::Move {
            center_hz: 7.4e6,
            span_hz: 480e3,
        })
        .unwrap();
        // the next line (100 ms at 10 lines/s) has the new bins
        run_jobs(&mut j, &tap, &mut e, t, 120, &tones);
        let l = newest(&mut rx);
        assert!((l.first_hz - (7.4e6 - 240e3)).abs() <= l.bin_hz);
        let p = peak(&l, 7.4e6);
        assert!((p + 20.0).abs() < 1.0, "{p} dB");
        assert_eq!(e.jobs.last().unwrap().k0, 239);
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
