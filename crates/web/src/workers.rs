// SPDX-License-Identifier: GPL-3.0-or-later
//! The sessions' DSP jobs on the pool (`doc/m3-multi-user.md` §3, §4):
//!
//! * [`AudioJob`]: a session's RX channel to PCM frames, and its narrow
//!   waterfall views computed from that channel (no WF slot needed).
//! * [`ViewJob`]: one shared hardware view: its WF blocks into one FFT
//!   engine, lines re-binned and sent to each session watching it.
//!
//! They hand encoded frames to the sessions' WebSocket tasks; a full queue
//! drops the frame (counted), it never blocks.

use crate::proto::{
    self, FLAG_GAP, FLAG_NEW_GEN, FLAG_OVERLOAD, FLAG_PACKED, FLAG_STALE, FLAG_STEREO,
};
use crate::wideaudio::{self, BinSource};
use crossbeam_channel::{Receiver as CbReceiver, TryRecvError};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::error::TrySendError;
use web888_core::pool::{Ctx, Job};
use web888_core::wf::SessionId;
use web888_dsp::C32;
use web888_dsp::audio::{AudioChain, AudioSpec, audio_rate_for};
use web888_dsp::binrx::BinTap;
use web888_dsp::chan::Droop;
use web888_dsp::wf::{WF_DB_STEP, WF_DB0, WfEngine, WfLine, WfSpec, quantize};
use web888_radio::{RxBlock, WfBlock};

/// Audio frame length: 20 ms at 12 kS/s (twice that at 24 kS/s: the same
/// 20 ms).
pub const AUDIO_FRAME: usize = 240;

/// Frames to the WebSocket task, with a drop counter.
#[derive(Clone)]
pub struct Out {
    pub tx: Sender<Vec<u8>>,
    pub drops: Arc<AtomicU64>,
    /// Also counts the drops (all sessions of a protocol, the metrics).
    pub total: Option<&'static AtomicU64>,
}

impl Out {
    /// False when the session is gone.
    pub(crate) fn send(&self, frame: Vec<u8>) -> bool {
        match self.tx.try_send(frame) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                self.drops.fetch_add(1, Ordering::Relaxed);
                if let Some(t) = self.total {
                    t.fetch_add(1, Ordering::Relaxed);
                }
                true
            }
            Err(TrySendError::Closed(_)) => false,
        }
    }
}

/// A session's waterfall output: its frame queue and line numbering
/// (shared by whichever job draws its view).
#[derive(Clone)]
pub struct WfSink {
    pub out: Out,
    pub seq: Arc<AtomicU32>,
    /// Kiwi protocol lines instead of web888d's.
    pub kiwi: Option<Arc<crate::kiwi::wire::WfEnc>>,
    /// Lines packed (`wfpack`) when the client asked.
    pack: Arc<std::sync::atomic::AtomicBool>,
    packer: Arc<std::sync::Mutex<crate::wfpack::WfPack>>,
}

impl WfSink {
    pub fn new(out: Out) -> Self {
        WfSink {
            out,
            seq: Arc::default(),
            kiwi: None,
            pack: Arc::default(),
            packer: Arc::default(),
        }
    }

    /// Pack this session's lines (`wf`'s `pack`).
    pub fn set_pack(&self, on: bool) {
        self.pack.store(on, Ordering::Relaxed);
    }

    /// Send levels `db` (dBFS) for bins from `f0` (Hz) every `df`.
    pub(crate) fn send(&self, new_gen: bool, fresh: bool, f0: f64, df: f64, db: &[f32]) -> bool {
        if let Some(k) = &self.kiwi {
            return self.out.send(k.encode(f0, df, db));
        }
        let mut flags = if new_gen { FLAG_NEW_GEN } else { 0 };
        if !fresh {
            flags |= FLAG_STALE;
        }
        let levels = quantize(db);
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        if self.pack.load(Ordering::Relaxed) {
            let mut p = self.packer.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(packed) = p.pack(&levels) {
                let f = proto::wf_frame_payload(
                    flags | FLAG_PACKED,
                    seq,
                    f0,
                    df,
                    WF_DB0,
                    WF_DB_STEP,
                    unix_ms(),
                    levels.len(),
                    packed,
                );
                drop(p);
                return self.out.send(f);
            }
        }
        let f = proto::wf_frame(flags, seq, f0, df, WF_DB0, WF_DB_STEP, unix_ms(), &levels);
        self.out.send(f)
    }
}

fn unix_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1e3)
}

pub(crate) fn period(fps: f64) -> Duration {
    Duration::from_secs_f64(1.0 / fps.clamp(0.5, 60.0))
}

/// FFT budget and line period under shedding level `shed`: the budget
/// halves per level down to 1, then the rate does.
pub(crate) fn shed(budget: usize, fps: f64, level: u32) -> (usize, Duration) {
    let steps = usize::BITS - budget.max(1).leading_zeros() - 1;
    let b = (budget >> level.min(steps)).max(1);
    let slow = level.saturating_sub(steps).min(4);
    (b, period(fps) * (1 << slow))
}

/// Merge display bins in groups of `g` (power average).
pub(crate) fn rebin(db: &[f32], g: usize) -> Vec<f32> {
    web888_dsp::wf::rebin_db(db, g)
}

/// A narrow view computed from the session's RX channel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChanView {
    /// The channel's tuning (dial).
    pub dial_hz: f64,
    pub center_hz: f64,
    pub span_hz: f64,
    pub bins: usize,
    pub fps: f64,
    pub budget: usize,
}

struct ChanWf {
    view: ChanView,
    engine: WfEngine,
    next: Instant,
    new_gen: bool,
    contiguous: bool,
}

/// Changes for a session's audio job.
#[derive(Clone, Copy, Debug)]
pub enum AudioCmd {
    Spec(AudioSpec),
    /// The RX channel carrying the session's lease (none: no audio).
    Channel(Option<usize>),
    /// Draw the session's waterfall from its channel (or stop).
    Wf(Option<ChanView>),
    /// Kiwi protocol packets: their settings.
    Kiwi(crate::kiwi::wire::SndSet),
    /// The dial (Hz): for audio from the bins (`wideaudio`).
    Dial(f64),
    /// How web frames carry the audio (`codec`).
    Codec(crate::codec::Codec),
    /// Frames carry their time (`FLAG_TIME`).
    Timestamps(bool),
}

/// How a job packs its audio.
pub enum Framing {
    /// web888d's frames (`proto::audio_frame`, or another `codec`), 20 ms
    /// each.
    Web(Box<crate::codec::WebEnc>),
    /// Kiwi `SND` packets.
    Kiwi(Box<crate::kiwi::wire::SndEnc>),
}

/// A session's channel on the DSP pool: PCM frames, and the waterfall
/// when its view lies inside the channel. Ends when the session drops its
/// command sender or its frame queue.
pub struct AudioJob {
    ch: Option<usize>,
    rx_rate: f64,
    droop: Droop,
    cmd: CbReceiver<AudioCmd>,
    out: Out,
    framing: Framing,
    wf_sink: WfSink,
    chain: AudioChain,
    pcm: Vec<f32>,
    next_frame: Option<u64>,
    flags: u8,
    seq: u32,
    rssi: f32,
    wf: Option<ChanWf>,
    /// The bins, for an audio rate the RX channel can't give.
    tap: Option<BinTap>,
    dial: Option<f64>,
    /// The spec as asked (the chain's may have fallen back to 12 kS/s).
    asked: AudioSpec,
    src: Option<BinSource>,
    wide: Vec<C32>,
    /// The frames' times: the RX stream's timebase, and where the chain's
    /// samples came from.
    clock: Option<Arc<web888_hw::HwStatus>>,
    times: Option<Times>,
}

/// Which input sample was when, for the chain's output (`doc/api.md`
/// §8.5): the chain makes output sample `j` from input sample
/// `j * input_rate / rate`, `delay_s` earlier.
#[derive(Default)]
struct Times {
    /// Input samples since the chain (re)started.
    inputs: u64,
    /// Output samples (per channel) since then.
    outputs: i64,
    /// Runs of input: (first input index, its UTC, samples per second).
    runs: VecDeque<(u64, f64, f64)>,
}

impl Times {
    fn add(&mut self, n: usize, utc: Option<f64>, rate: f64) {
        if let Some(t) = utc {
            self.runs.push_back((self.inputs, t, rate));
        }
        self.inputs += n as u64;
    }

    /// UTC of output sample `j` (per channel) at the antenna.
    fn utc(&self, j: i64, chain: &AudioChain) -> Option<f64> {
        let x = j as f64 * chain.input_rate() / chain.spec().rate;
        let run = self
            .runs
            .iter()
            .rev()
            .find(|r| r.0 as f64 <= x)
            .or(self.runs.front())?;
        Some(run.1 + (x - run.0 as f64) / run.2 - chain.delay_s())
    }

    /// Forget runs before output sample `j`.
    fn keep_from(&mut self, j: i64, chain: &AudioChain) {
        let x = j as f64 * chain.input_rate() / chain.spec().rate;
        while self.runs.len() > 1 && self.runs[1].0 as f64 <= x {
            self.runs.pop_front();
        }
    }
}

impl AudioJob {
    pub fn new(
        ch: Option<usize>,
        rx_rate: f64,
        droop: Droop,
        spec: AudioSpec,
        cmd: CbReceiver<AudioCmd>,
        out: Out,
        wf_sink: WfSink,
    ) -> Self {
        AudioJob {
            ch,
            rx_rate,
            droop,
            cmd,
            out,
            framing: Framing::Web(Box::default()),
            wf_sink,
            chain: AudioChain::new(spec, rx_rate, droop),
            pcm: Vec::new(),
            next_frame: None,
            flags: 0,
            seq: 0,
            rssi: -200.0,
            wf: None,
            tap: None,
            dial: None,
            asked: spec,
            src: None,
            wide: Vec::new(),
            clock: None,
            times: None,
        }
    }

    /// Where the RX frames' times are kept (for timestamps).
    pub fn with_clock(mut self, status: Arc<web888_hw::HwStatus>) -> Self {
        self.clock = Some(status);
        self
    }

    /// The chain starts over: its samples' times too (the pending
    /// audio counts as just before).
    fn restart_times(&mut self) {
        // pcm[0] becomes output -(its length): before the new count
        if let Some(t) = &mut self.times {
            *t = Times::default();
        }
    }

    /// Take audio rates the RX channel can't give from these bins.
    pub fn with_tap(mut self, tap: Option<BinTap>) -> Self {
        self.tap = tap;
        self
    }

    fn make_chain(&self) -> AudioChain {
        match &self.src {
            Some(_) => AudioChain::new(self.asked, wideaudio::RATE, Droop::None),
            None => AudioChain::new(self.asked, self.rx_rate, self.droop),
        }
    }

    /// Read the bins when the asked rate needs them (and there are
    /// sources left; else the chain falls back to 12 kS/s).
    fn sync_src(&mut self) {
        let want =
            self.ch.is_some() && audio_rate_for(self.asked.rate, self.rx_rate) != self.asked.rate;
        let was = self.src.is_some();
        match (&self.tap, self.dial) {
            (Some(tap), Some(d)) if want => {
                let (lo, hi) = (self.asked.lo_hz, self.asked.hi_hz);
                match &mut self.src {
                    Some(s) if s.covers(d, lo, hi) => s.retune(d),
                    _ => {
                        self.src = None;
                        self.src = BinSource::new(tap, d, lo, hi);
                        self.flags |= FLAG_GAP;
                    }
                }
            }
            _ => self.src = None,
        }
        if was != self.src.is_some() {
            self.chain = self.make_chain();
            self.pcm.clear();
            self.flags |= FLAG_GAP;
            self.restart_times();
        }
    }

    /// Pack the audio as `f` instead of web888d's frames.
    pub fn with_framing(mut self, f: Framing) -> Self {
        self.framing = f;
        self
    }

    fn set_wf(&mut self, v: Option<ChanView>) {
        let Some(v) = v else {
            self.wf = None;
            return;
        };
        if let Some(w) = &mut self.wf {
            let s = w.engine.spec();
            if v.bins == w.view.bins && v.fps == w.view.fps && s.budget == v.budget.max(1) {
                // a retune or a shifted view: same engine, new place
                w.new_gen |= v.dial_hz != w.view.dial_hz;
                w.contiguous &= v.dial_hz == w.view.dial_hz;
                w.view = v;
                return;
            }
        }
        // FFT bins at least as fine as the view asks for
        let df = v.span_hz / v.bins as f64;
        let bins = ((self.rx_rate / df).ceil() as usize)
            .next_power_of_two()
            .clamp(256, 4096);
        self.wf = Some(ChanWf {
            view: v,
            engine: WfEngine::new(WfSpec {
                fs: self.rx_rate,
                bins,
                budget: v.budget.max(1),
                fps: v.fps,
                droop: self.droop,
                fft_per_bin: 2,
            }),
            next: Instant::now() + period(v.fps),
            new_gen: true,
            contiguous: false,
        });
    }

    /// Apply the session's changes; false when the session is gone.
    fn commands(&mut self) -> bool {
        loop {
            match self.cmd.try_recv() {
                Ok(AudioCmd::Spec(s)) => {
                    let ch = self.chain.channels();
                    self.asked = s;
                    self.sync_src();
                    self.chain.set_spec(s);
                    if self.chain.channels() != ch {
                        // mono and stereo samples don't mix
                        self.pcm.clear();
                    }
                    // the rate may have changed: count anew
                    self.restart_times();
                }
                Ok(AudioCmd::Channel(c)) if c != self.ch => {
                    // another channel's history: start over
                    self.ch = c;
                    self.sync_src();
                    self.chain = self.make_chain();
                    self.pcm.clear();
                    self.restart_times();
                    self.next_frame = None;
                    self.flags |= FLAG_GAP;
                    if let Some(w) = &mut self.wf {
                        w.contiguous = false;
                    }
                }
                Ok(AudioCmd::Channel(_)) => {}
                Ok(AudioCmd::Dial(hz)) => {
                    self.dial = Some(hz);
                    self.sync_src();
                }
                Ok(AudioCmd::Wf(v)) => self.set_wf(v),
                Ok(AudioCmd::Codec(c)) => {
                    if let Framing::Web(e) = &mut self.framing {
                        e.set_codec(c);
                    }
                }
                Ok(AudioCmd::Timestamps(on)) => {
                    self.times = on.then(Times::default);
                    self.restart_times();
                }
                Ok(AudioCmd::Kiwi(k)) => {
                    if let Framing::Kiwi(e) = &mut self.framing {
                        e.set(k);
                    }
                }
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
    }
}

impl Job for AudioJob {
    fn kind(&self) -> web888_core::pool::JobKind {
        web888_core::pool::JobKind::Audio
    }

    fn rx(&mut self, b: &RxBlock) -> bool {
        if !self.commands() {
            return false;
        }
        let Some(ch) = self.ch else {
            return true;
        };
        let contiguous = self.next_frame == Some(b.first_frame);
        if self.next_frame.is_some() && !contiguous {
            self.flags |= FLAG_GAP;
        }
        self.next_frame = Some(b.end_frame());
        if b.overload {
            self.flags |= FLAG_OVERLOAD;
        }
        if let Some(w) = &mut self.wf {
            w.engine.push_rx(b.channel(ch), w.contiguous && contiguous);
            w.contiguous = true;
        }
        let had = self.pcm.len();
        let tb = match (&self.times, &self.clock) {
            (Some(_), Some(c)) => {
                let tb = c.timebase.lock().unwrap();
                Some((
                    tb.utc(b.first_frame as f64),
                    tb.utc(b.end_frame() as f64),
                    tb.rate(),
                ))
            }
            _ => None,
        };
        match &mut self.src {
            Some(s) => {
                self.wide.clear();
                if s.read(&mut self.wide) {
                    self.flags |= FLAG_GAP;
                }
                if let (Some(t), Some((_, end, _))) = (&mut self.times, tb) {
                    // the bins' newest samples: about when the block came
                    let n = self.wide.len();
                    t.add(
                        n,
                        end.map(|e| e - n as f64 / wideaudio::RATE),
                        wideaudio::RATE,
                    );
                }
                self.chain.process_c32(self.wide.drain(..), &mut self.pcm);
            }
            None => {
                if let (Some(t), Some((first, _, rate))) = (&mut self.times, tb) {
                    t.add(b.frames(), first, rate.unwrap_or(self.rx_rate));
                }
                self.chain.process(b.channel(ch), &mut self.pcm)
            }
        }
        // frames of AUDIO_FRAME sample times (Kiwi: its packet), of every
        // channel
        let chans = self.chain.channels();
        if let Some(t) = &mut self.times {
            t.outputs += ((self.pcm.len() - had) / chans.max(1)) as i64;
        }
        let per_12k = (self.chain.spec().rate / web888_dsp::audio::AUDIO_RATE).round() as usize;
        let len = chans
            * match &self.framing {
                Framing::Web(_) => AUDIO_FRAME * per_12k,
                Framing::Kiwi(e) => e.samples(chans),
            };
        let stereo = if chans == 2 { FLAG_STEREO } else { 0 };
        let squelched = self.chain.squelched();
        let mut used = 0;
        while self.pcm.len() - used >= len {
            // a block may finish several frames: they share its reading
            self.rssi = self.chain.take_power_db().unwrap_or(self.rssi);
            let pcm = &self.pcm[used..used + len];
            // this frame's first sample, counted as the chain's outputs
            let utc = self.times.as_ref().and_then(|t| {
                let j = t.outputs - ((self.pcm.len() - used) / chans.max(1)) as i64;
                t.utc(j, &self.chain)
            });
            let f = match &mut self.framing {
                Framing::Web(e) => e.frame(
                    self.flags | stereo,
                    self.seq,
                    self.chain.spec().rate as u32,
                    self.rssi,
                    pcm,
                    chans,
                    self.chain.spec(),
                ),
                Framing::Kiwi(e) => e.encode(
                    self.flags & FLAG_OVERLOAD != 0,
                    squelched,
                    self.rssi,
                    pcm,
                    chans,
                ),
            };
            let f = match (utc, &self.framing) {
                (Some(t), Framing::Web(_)) => proto::timed(f, t),
                _ => f,
            };
            if !self.out.send(f) {
                return false;
            }
            self.flags = 0;
            self.seq = self.seq.wrapping_add(1);
            used += len;
        }
        self.pcm.drain(..used);
        if let Some(t) = &mut self.times {
            let j = t.outputs - (self.pcm.len() / chans.max(1)) as i64;
            t.keep_from(j, &self.chain);
        }
        true
    }

    fn tick(&mut self, now: Instant, ctx: Ctx) -> bool {
        if !self.commands() || self.out.tx.is_closed() {
            return false;
        }
        let Some(w) = &mut self.wf else { return true };
        if now < w.next || self.ch.is_none() {
            return true;
        }
        let (budget, p) = shed(w.view.budget, w.view.fps, ctx.shed);
        w.engine.set_budget(budget);
        w.next = (w.next + p).max(now);
        let Some(line) = w.engine.line() else {
            return true;
        };
        // the bins inside the view, relative to the dial
        let v = w.view;
        let lo = v.center_hz - v.span_hz / 2.0 - v.dial_hz;
        let hi = v.center_hz + v.span_hz / 2.0 - v.dial_hz;
        let k0 = ((lo - line.first_offset_hz) / line.bin_hz).ceil().max(0.0) as usize;
        let k1 =
            (((hi - line.first_offset_hz) / line.bin_hz).floor() as usize + 1).min(line.db.len());
        if k0 >= k1 {
            return true;
        }
        let f0 = v.dial_hz + line.first_offset_hz + k0 as f64 * line.bin_hz;
        let new_gen = std::mem::take(&mut w.new_gen);
        self.wf_sink
            .send(new_gen, line.fresh, f0, line.bin_hz, &line.db[k0..k1])
    }
}

/// A session watching a hardware view.
pub struct ViewSub {
    pub session: SessionId,
    pub bins: usize,
    pub fps: f64,
    pub sink: WfSink,
    /// The session's first line from this view (it came from another).
    pub new_gen: bool,
}

struct Sub {
    s: ViewSub,
    next: Instant,
}

pub enum ViewCmd {
    /// Add a session, or update its bins and rate.
    Add(ViewSub),
    Remove(SessionId),
    /// Show another part of the band (a view from the bins whose only
    /// session moved it: its FFTs carry on).
    Move {
        center_hz: f64,
        span_hz: f64,
    },
}

/// One hardware view shared by its sessions, captured at twice its span
/// (`decim` is the capture's) and showing the middle half. Ends when the
/// service drops its command sender (the view is gone).
pub struct ViewJob {
    generation: u64,
    center_hz: f64,
    adc_hz: f64,
    decim: u16,
    budget: usize,
    cmd: CbReceiver<ViewCmd>,
    subs: Vec<Sub>,
    engine: Option<WfEngine>,
    prev: Option<Arc<WfBlock>>,
    next: Instant,
}

impl ViewJob {
    pub fn new(
        generation: u64,
        center_hz: f64,
        adc_hz: f64,
        decim: u16,
        budget: usize,
        cmd: CbReceiver<ViewCmd>,
    ) -> Self {
        ViewJob {
            generation,
            center_hz,
            adc_hz,
            decim,
            budget: budget.max(1),
            cmd,
            subs: Vec::new(),
            engine: None,
            prev: None,
            next: Instant::now(),
        }
    }

    fn bins(&self) -> usize {
        self.subs.iter().map(|s| s.s.bins).max().unwrap_or(1024)
    }

    fn fps(&self) -> f64 {
        self.subs.iter().map(|s| s.s.fps).fold(1.0, f64::max)
    }

    /// The engine for the subscribers' finest bins and fastest rate.
    fn fit_engine(&mut self) {
        let (bins, fps) = (self.bins(), self.fps());
        if self
            .engine
            .as_ref()
            .is_some_and(|e| e.spec().bins == 2 * bins && e.spec().fps == fps)
        {
            return;
        }
        self.engine = Some(WfEngine::new(WfSpec {
            fs: self.adc_hz / self.decim as f64,
            // twice the bins, the middle half shown; one FFT bin per
            // shown bin, so the FFT isn't bigger than without the crop
            bins: 2 * bins,
            budget: self.budget,
            fps,
            droop: Droop::Wf {
                adc_hz: self.adc_hz,
                decim: self.decim,
            },
            fft_per_bin: 1,
        }));
        self.prev = None;
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
                // hardware views are captured where they are
                Ok(ViewCmd::Move { .. }) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
        if changed && !self.subs.is_empty() {
            self.fit_engine();
        }
        true
    }

    fn send(&mut self, line: &WfLine, now: Instant, p: Duration) {
        // the middle half of the captured band
        let lo = line.db.len() / 4;
        let bins = line.db.len() / 2;
        let mid = &line.db[lo..lo + bins];
        let first = line.first_offset_hz + lo as f64 * line.bin_hz;
        let center = self.center_hz;
        self.subs.retain_mut(|x| {
            if now < x.next {
                return true;
            }
            x.next = (x.next + p.max(period(x.s.fps))).max(now);
            let g = (bins / x.s.bins.max(1)).max(1);
            let db = rebin(mid, g);
            let f0 = center + first + (g as f64 - 1.0) / 2.0 * line.bin_hz;
            let new_gen = std::mem::take(&mut x.s.new_gen);
            x.s.sink
                .send(new_gen, line.fresh, f0, line.bin_hz * g as f64, &db)
        });
    }
}

impl Job for ViewJob {
    fn kind(&self) -> web888_core::pool::JobKind {
        web888_core::pool::JobKind::Waterfall
    }

    fn wf(&mut self, b: &Arc<WfBlock>) -> bool {
        if b.generation != self.generation {
            return true;
        }
        if let Some(e) = &mut self.engine {
            let contiguous = self.prev.as_ref().is_some_and(|p| b.continues(p));
            e.push(&b.iq, contiguous);
            self.prev = Some(b.clone());
        }
        true
    }

    fn tick(&mut self, now: Instant, ctx: Ctx) -> bool {
        if !self.commands(now) {
            return false;
        }
        if now < self.next || self.subs.is_empty() {
            return true;
        }
        let (budget, p) = shed(self.budget, self.fps(), ctx.shed);
        self.next = (self.next + p).max(now);
        let Some(e) = &mut self.engine else {
            return true;
        };
        e.set_budget(budget);
        if let Some(line) = e.line() {
            self.send(&line, now, p);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shedding_halves_the_budget_then_the_rate() {
        let p = period(20.0);
        assert_eq!(shed(30, 20.0, 0), (30, p));
        assert_eq!(shed(30, 20.0, 1), (15, p));
        // 30 = 0b11110: four halvings to 1
        assert_eq!(shed(30, 20.0, 4), (1, p));
        assert_eq!(shed(30, 20.0, 5), (1, p * 2));
        assert_eq!(shed(30, 20.0, 99), (1, p * 16));
    }

    #[test]
    fn rebin_averages_power() {
        let db = [-10.0, -10.0, -20.0, -30.0];
        let r = rebin(&db, 2);
        assert!((r[0] + 10.0).abs() < 2e-3);
        // (0.01 + 0.001) / 2 -> -22.6 dB
        assert!((r[1] + 22.596).abs() < 3e-3, "{r:?}");
        assert_eq!(rebin(&db, 1), db);
    }
}
