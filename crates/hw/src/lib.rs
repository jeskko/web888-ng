// SPDX-License-Identifier: GPL-3.0-or-later
//! The hardware thread: sole owner of the `Radio`. Applies control commands,
//! reads RX frames and waterfall samples and fans them out to subscribers,
//! and runs the PPS clock loop.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::time::{Duration, Instant};
use web888_radio::clock::AdcClock;
use web888_radio::*;

pub mod wf;
pub use wf::{Rotation, WfView};

/// Commands for the hw thread, from any consumer (HPSDR, web sessions).
pub enum HwCmd {
    Tune {
        ch: usize,
        hz: f64,
    },
    /// Channel `ch` is free (`Radio::idle_rx`).
    IdleRx {
        ch: usize,
    },
    Atten {
        db: f32,
    },
    /// What WF slot `slot` captures: nothing, one view continuously, or
    /// several in turn as one-shot blocks (`wf::Rotation`). A view kept
    /// from the previous plan of a continuous slot isn't restarted.
    WfPlan {
        slot: usize,
        views: Vec<WfView>,
    },
    SubscribeRx(Subscriber<RxBlock>),
    SubscribeWf(Subscriber<WfBlock>),
}

/// A consumer's end of the fan-out: a bounded queue that loses blocks
/// (counted) instead of stalling the hw thread. It is dropped from the
/// fan-out when its receiver goes away.
pub struct Subscriber<T> {
    tx: SyncSender<Arc<T>>,
    dropped: Arc<AtomicU64>,
    /// Blocks queued for it (its receiver counts what it took: the
    /// difference is how far behind it is).
    sent: Arc<AtomicU64>,
}

impl<T> Subscriber<T> {
    /// A subscriber queueing up to `depth` blocks, its receiver and its
    /// drop counter.
    pub fn new(depth: usize) -> (Self, Receiver<Arc<T>>, Arc<AtomicU64>) {
        let (tx, rx) = sync_channel(depth);
        let dropped = Arc::new(AtomicU64::new(0));
        (
            Subscriber {
                tx,
                dropped: dropped.clone(),
                sent: Arc::new(AtomicU64::new(0)),
            },
            rx,
            dropped,
        )
    }

    /// The count of blocks queued for this subscriber.
    pub fn sent(&self) -> Arc<AtomicU64> {
        self.sent.clone()
    }
}

/// Hands each block to every subscriber.
pub struct Fanout<T> {
    subs: Vec<Subscriber<T>>,
}

impl<T> Default for Fanout<T> {
    fn default() -> Self {
        Fanout { subs: Vec::new() }
    }
}

impl<T> Fanout<T> {
    pub fn add(&mut self, s: Subscriber<T>) {
        self.subs.push(s);
    }

    pub fn send(&mut self, block: T) {
        if self.subs.is_empty() {
            return;
        }
        let b = Arc::new(block);
        self.subs.retain(|s| match s.tx.try_send(b.clone()) {
            Ok(()) => {
                s.sent.fetch_add(1, Ordering::Relaxed);
                true
            }
            Err(TrySendError::Full(_)) => {
                s.dropped.fetch_add(1, Ordering::Relaxed);
                true
            }
            Err(TrySendError::Disconnected(_)) => false,
        });
    }
}

/// Detects lost RX frames by comparing what arrived with the configured
/// rate over a window. The stock driver reports no ring overruns, so with
/// it a reader that falls behind (slow CPU, a stalled thread) only shows
/// up here; the open driver also counts them (`Radio::hw_stats`).
pub struct RateMonitor {
    window: Duration,
    start: Option<Instant>,
    r2: u16,
    frames: u64,
}

/// One finished window: frames per second seen and expected.
#[derive(Debug, PartialEq)]
pub struct RateReport {
    pub got: f64,
    pub want: f64,
}

impl RateReport {
    /// Missing share of the expected frames (0.0 = none lost).
    pub fn deficit(&self) -> f64 {
        1.0 - self.got / self.want
    }
}

impl RateMonitor {
    pub fn new(window: Duration) -> Self {
        RateMonitor {
            window,
            start: None,
            r2: 0,
            frames: 0,
        }
    }

    /// Account `frames` read at `now`; returns a report when a window ends.
    /// A rate change (`r2`) restarts the window.
    pub fn add(&mut self, now: Instant, frames: u64, r2: u16, adc_hz: f64) -> Option<RateReport> {
        let start = match self.start {
            Some(t) if r2 == self.r2 => t,
            _ => {
                // first frames after (re)start: begin counting from here
                self.start = Some(now);
                self.r2 = r2;
                self.frames = 0;
                return None;
            }
        };
        self.frames += frames;
        let dt = now.duration_since(start);
        if dt < self.window {
            return None;
        }
        let secs = dt.as_secs_f64();
        let r = RateReport {
            got: self.frames as f64 / secs,
            want: adc_hz / (256.0 * r2 as f64),
        };
        self.start = Some(now);
        self.frames = 0;
        Some(r)
    }
}

/// RX data is read in batches of about this much signal time. The PL
/// delivers 128-byte bursts, so reading whatever is there makes thousands
/// of tiny reads per second.
const RX_BATCH: Duration = Duration::from_millis(5);

/// Pause after a read of `got` frames: the time the rest of a batch takes
/// to arrive at `frames_per_s` (none when the reader is behind).
pub fn read_pause(got: usize, frames_per_s: f64) -> Duration {
    let batch = RX_BATCH.as_secs_f64();
    let missing = batch - got as f64 / frames_per_s;
    Duration::from_secs_f64(missing.clamp(0.0, batch))
}

/// Health of the hw thread, readable by any consumer (status displays).
#[derive(Debug, Default)]
pub struct HwStatus {
    /// RX blocks read while the ADC overflowed.
    pub overload_blocks: AtomicU64,
    /// PPS-corrected ADC clock error, ppb (0 without PPS).
    pub clock_ppb: std::sync::atomic::AtomicI64,
    /// The last rate window delivered the expected RX frames.
    pub rx_rate_ok: AtomicBool,
    pub rx_frames: AtomicU64,
}

pub struct HwLoop {
    pub radio: Box<dyn Radio>,
    pub ctl: Receiver<HwCmd>,
    pub stop: Arc<AtomicBool>,
    pub status: Arc<HwStatus>,
    pub verbose: bool,
}

/// A WF slot's capture.
#[derive(Default)]
struct Slot {
    /// One view, streamed continuously.
    cont: Option<WfView>,
    /// Several views, one-shot blocks in turn.
    rot: Rotation,
    /// The block being captured.
    shot: Option<Shot>,
    /// Blocks per view (by generation) so far.
    seq: std::collections::HashMap<u64, u64>,
    /// `Radio::wf_gaps` already accounted for.
    gaps_seen: u64,
    /// Mark the next block as following a gap (restart after a retune).
    force_gap: bool,
    /// Samples a continuous slot may still read (`WF_MAX_RATE`), and when
    /// that was worked out.
    allowance: f64,
    refilled: Option<Instant>,
}

struct Shot {
    view: WfView,
    got: usize,
    /// Give up on a block that doesn't complete by then.
    deadline: Instant,
}

impl Slot {
    fn is_idle(&self) -> bool {
        self.cont.is_none() && self.rot.views().next().is_none()
    }

    fn next_seq(&mut self, generation: u64) -> u64 {
        let s = self.seq.entry(generation).or_insert(0);
        *s += 1;
        *s - 1
    }
}

/// WF samples per read; one read per slot and loop pass at most, and a
/// continuous slot no more than `WF_MAX_RATE`: reading a wide span's full
/// rate (61 MS/s at decimation 2) would only cost the RX reads their time.
const WF_READ: usize = 16384;
const WF_READS_PER_PASS: usize = 1;
/// Most WF samples a continuous slot reads per second. A line uses the
/// FFT budget's worth of samples (30 x 2048 at 20 lines/s = 1.2 MS/s);
/// a wider span loses the rest in the ring, and a backend computing its
/// WF in software (iq) then doesn't compute it.
const WF_MAX_RATE: f64 = 1.5e6;

impl HwLoop {
    fn plan_wf(&mut self, slot: usize, w: &mut Slot, views: Vec<WfView>) -> Result<()> {
        let now = Instant::now();
        w.seq
            .retain(|g, _| views.iter().any(|v| v.generation == *g));
        match views.as_slice() {
            [] => {
                if let Some(v) = w.cont.take() {
                    // a one-shot block ends the stream on every backend
                    self.radio.start_wf(slot, v.hz, v.decim, WfMode::OneShot)?;
                }
                w.rot.set(&[], now);
                w.shot = None;
            }
            [v] => {
                w.rot.set(&[], now);
                w.shot = None;
                let same = w.cont.is_some_and(|c| {
                    (c.generation, c.hz, c.decim) == (v.generation, v.hz, v.decim)
                });
                if !same {
                    self.radio
                        .start_wf(slot, v.hz, v.decim, WfMode::Continuous)?;
                    w.gaps_seen = 0;
                    w.force_gap = true;
                }
                w.cont = Some(*v);
            }
            _ => {
                if let Some(v) = w.cont.take() {
                    self.radio.start_wf(slot, v.hz, v.decim, WfMode::OneShot)?;
                }
                w.rot.set(&views, now);
                // a block of a view that's gone is abandoned
                if w.shot
                    .as_ref()
                    .is_some_and(|s| !views.iter().any(|v| v.generation == s.view.generation))
                {
                    w.shot = None;
                }
            }
        }
        Ok(())
    }

    /// Read what a WF slot has and fan it out; start the next one-shot
    /// block of a time-shared slot.
    fn read_wf_slot(
        &mut self,
        slot: usize,
        w: &mut Slot,
        block: usize,
        buf: &mut [Iq16],
        out: &mut Fanout<WfBlock>,
    ) -> Result<()> {
        if w.is_idle() {
            return Ok(());
        }
        if let Some(v) = w.cont {
            let now = Instant::now();
            let dt = w
                .refilled
                .map_or(0.0, |t| now.duration_since(t).as_secs_f64());
            w.refilled = Some(now);
            // at most one read's worth banked, so no bursts
            w.allowance = (w.allowance + dt * WF_MAX_RATE).min(buf.len() as f64);
            for _ in 0..WF_READS_PER_PASS {
                if w.allowance < buf.len() as f64 {
                    break;
                }
                // gaps counted by now lie before this read's samples
                let gaps = self.radio.wf_gaps(slot);
                let got = self.radio.read_wf(slot, buf)?;
                if got == 0 {
                    break;
                }
                w.allowance -= got as f64;
                let gap_before = std::mem::take(&mut w.force_gap) || gaps != w.gaps_seen;
                w.gaps_seen = gaps;
                out.send(WfBlock {
                    slot,
                    generation: v.generation,
                    seq: w.next_seq(v.generation),
                    gap_before,
                    iq: buf[..got].to_vec(),
                });
                if got < buf.len() {
                    break;
                }
            }
            return Ok(());
        }
        let now = Instant::now();
        if let Some(s) = &mut w.shot {
            let got = self.radio.read_wf(slot, buf)?;
            if got > 0 {
                let first = s.got == 0;
                s.got += got;
                let g = s.view.generation;
                let seq = w.next_seq(g);
                out.send(WfBlock {
                    slot,
                    generation: g,
                    seq,
                    // every block is a fresh capture
                    gap_before: first,
                    iq: buf[..got].to_vec(),
                });
            }
            let s = w.shot.as_ref().expect("shot");
            if s.got >= block || now > s.deadline {
                w.shot = None;
            } else {
                return Ok(());
            }
        }
        if let Ok(v) = w.rot.next(now) {
            self.radio.start_wf(slot, v.hz, v.decim, WfMode::OneShot)?;
            let t = block as f64 * v.decim as f64 / self.radio.adc_hz();
            w.shot = Some(Shot {
                view: v,
                got: 0,
                deadline: now + Duration::from_secs_f64(2.0 * t + 0.2),
            });
        }
        Ok(())
    }

    pub fn run(mut self) -> Result<()> {
        let caps = self.radio.caps();
        let n = caps.rx_channels;
        let mut tuned: Vec<Option<f64>> = vec![None; n];
        let mut wf: Vec<Slot> = (0..caps.wf_channels).map(|_| Slot::default()).collect();
        let mut rx_out = Fanout::<RxBlock>::default();
        let mut wf_out = Fanout::<WfBlock>::default();
        let mut clock = AdcClock::new(caps.adc_hz);
        let mut buf = vec![Iq32::default(); 512 * n];
        let mut wf_buf = vec![Iq16::default(); WF_READ];
        let mut frames = 0u64;
        let mut last_report = Instant::now();
        let mut fe = FrontEnd::default();
        // the board as `fe` says from the start (attenuator, RF path, ADC
        // clock source): GPIOs left by another program stay otherwise (on
        // the external clock input without a reference, the ADC runs at
        // ~188 MHz)
        self.radio.front_end(fe)?;
        let mut rate = RateMonitor::new(Duration::from_secs(10));
        let mut last_stats = self.radio.hw_stats()?;
        self.status.rx_rate_ok.store(true, Ordering::Relaxed);

        while !self.stop.load(Ordering::Relaxed) {
            while let Ok(c) = self.ctl.try_recv() {
                let res = match c {
                    HwCmd::Tune { ch, hz } => self.radio.tune_rx(ch, hz).map(|()| {
                        tuned[ch] = Some(hz);
                    }),
                    HwCmd::IdleRx { ch } => self.radio.idle_rx(ch).map(|()| {
                        tuned[ch] = None;
                    }),
                    HwCmd::Atten { db } => {
                        fe.atten_db = db.clamp(0.0, 31.5);
                        self.radio.front_end(fe)
                    }
                    HwCmd::WfPlan { slot, views } if slot < wf.len() => {
                        self.plan_wf(slot, &mut wf[slot], views)
                    }
                    HwCmd::WfPlan { slot, .. } => Err(Error::InvalidChannel(slot)),
                    HwCmd::SubscribeRx(s) => {
                        rx_out.add(s);
                        Ok(())
                    }
                    HwCmd::SubscribeWf(s) => {
                        wf_out.add(s);
                        Ok(())
                    }
                };
                // a bad request from a client must not stop the radio
                if let Err(e) = res {
                    eprintln!("hw: command refused: {e}");
                }
            }

            let got = self.radio.read_rx(&mut buf)?;
            if let Some(r) = rate.add(
                Instant::now(),
                got as u64,
                self.radio.rx_r2(),
                self.radio.adc_hz(),
            ) {
                let ok = r.deficit() <= 0.01;
                self.status.rx_rate_ok.store(ok, Ordering::Relaxed);
                if self.verbose || !ok {
                    eprintln!(
                        "rx: {:.0} frames/s, expected {:.0} ({:+.1} %){}",
                        r.got,
                        r.want,
                        -100.0 * r.deficit(),
                        if ok {
                            ""
                        } else {
                            ": frames lost, the reader is falling behind"
                        }
                    );
                }
            }
            let mut overload = false;
            if got > 0
                && let Some(s) = self.radio.hw_stats()?
            {
                if let Some(l) = last_stats {
                    overload = s.adc_overflows != l.adc_overflows;
                    if s.rx_overruns != l.rx_overruns {
                        eprintln!(
                            "rx: ring overrun, {} bytes skipped: the reader fell behind",
                            s.rx_overrun_bytes.wrapping_sub(l.rx_overrun_bytes)
                        );
                    }
                    // reset at each RX start
                    if s.rx_frames_dropped > l.rx_frames_dropped {
                        eprintln!(
                            "rx: {} frames dropped in the FPGA FIFO",
                            s.rx_frames_dropped - l.rx_frames_dropped
                        );
                    }
                }
                last_stats = Some(s);
            }
            if overload {
                self.status.overload_blocks.fetch_add(1, Ordering::Relaxed);
            }
            if got > 0 {
                self.status
                    .rx_frames
                    .fetch_add(got as u64, Ordering::Relaxed);
                rx_out.send(RxBlock {
                    first_frame: frames,
                    nch: n,
                    iq: buf[..got * n].to_vec(),
                    overload,
                });
                frames += got as u64;
            }

            for (slot, w) in wf.iter_mut().enumerate() {
                self.read_wf_slot(slot, w, caps.wf_block, &mut wf_buf, &mut wf_out)?;
            }

            let fps = self.radio.adc_hz() / (256.0 * self.radio.rx_r2() as f64);
            let pause = read_pause(got, fps);
            if !pause.is_zero() {
                std::thread::sleep(pause);
            }

            if let Some(p) = self.radio.pps()?
                && let Some(hz) = clock.on_pps(p)
            {
                self.radio.set_adc_hz(hz);
                self.status.clock_ppb.store(
                    ((hz / caps.adc_hz - 1.0) * 1e9).round() as i64,
                    Ordering::Relaxed,
                );
                for (ch, f) in tuned.iter().enumerate() {
                    if let Some(f) = f {
                        self.radio.tune_rx(ch, *f)?;
                    }
                }
                for (slot, w) in wf.iter_mut().enumerate() {
                    if let Some(v) = w.cont {
                        self.radio
                            .start_wf(slot, v.hz, v.decim, WfMode::Continuous)?;
                        w.gaps_seen = 0;
                        w.force_gap = true;
                    }
                    // a block being captured finishes at the old tuning
                }
                eprintln!(
                    "clock: ADC {:.3} Hz ({:+.3} ppm), channels retuned",
                    hz,
                    (hz / caps.adc_hz - 1.0) * 1e6
                );
            }

            if self.verbose && last_report.elapsed() >= Duration::from_secs(10) {
                last_report = Instant::now();
                eprintln!("hw: {frames} frames");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use web888_sim::{Pacing, Scene, SimConfig, SimRadio};

    const F0: f64 = 7_074_000.0;

    fn start(
        scene: Scene,
    ) -> (
        std::sync::mpsc::Sender<HwCmd>,
        Arc<AtomicBool>,
        std::thread::JoinHandle<Result<()>>,
    ) {
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let hw = HwLoop {
            radio: Box::new(SimRadio::new(SimConfig::hf(scene, Pacing::RealTime))),
            ctl: rx,
            stop: stop.clone(),
            status: Arc::default(),
            verbose: false,
        };
        let t = std::thread::spawn(move || hw.run());
        (tx, stop, t)
    }

    #[test]
    fn rx_fanout_survives_a_stalled_subscriber() {
        let (tx, stop, t) = start(Scene::new().tone(F0, -20.0));
        let (a, a_rx, a_drops) = Subscriber::new(64);
        let (b, b_rx, _) = Subscriber::new(64);
        let (c, _c_rx, c_drops) = Subscriber::new(1); // never read
        for s in [a, b, c] {
            tx.send(HwCmd::SubscribeRx(s)).unwrap();
        }
        let mut next = None;
        for _ in 0..40 {
            let x = a_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            let y = b_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(x, y);
            assert!(x.frames() > 0 && x.nch == 12);
            if let Some(n) = next {
                assert_eq!(x.first_frame, n, "the readers lost no block");
            }
            next = Some(x.end_frame());
        }
        stop.store(true, Ordering::Relaxed);
        t.join().unwrap().unwrap();
        assert_eq!(a_drops.load(Ordering::Relaxed), 0);
        assert!(c_drops.load(Ordering::Relaxed) > 0);
    }

    /// Mean phase step per sample of a complex stream (tone frequency / fs).
    fn cycles_per_sample(iq: &[Iq16]) -> f64 {
        let z: Vec<num_complex::Complex64> = iq
            .iter()
            .map(|s| num_complex::Complex64::new(s.i as f64, s.q as f64))
            .collect();
        let acc: num_complex::Complex64 = z.windows(2).map(|w| w[1] * w[0].conj()).sum();
        acc.arg() / (2.0 * std::f64::consts::PI)
    }

    #[test]
    fn wf_streams_with_generation_and_sequence() {
        // decim 128: fs 960 kHz (under the read cap, so nothing is lost);
        // the tone sits fs/8 above the centre
        let fs = ADC_HZ_HF / 128.0;
        let (tx, stop, t) = start(Scene::new().tone(F0 + fs / 8.0, -10.0));
        let (s, rx, _) = Subscriber::new(256);
        tx.send(HwCmd::SubscribeWf(s)).unwrap();
        let wf_start = |generation| HwCmd::WfPlan {
            slot: 0,
            views: vec![WfView {
                hz: F0,
                decim: 128,
                generation,
                fps: 20.0,
            }],
        };
        tx.send(wf_start(7)).unwrap();
        let mut prev: Option<Arc<WfBlock>> = None;
        let mut samples = 0;
        while samples < 100_000 {
            let b = rx.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!((b.slot, b.generation), (0, 7));
            match &prev {
                None => assert_eq!(b.seq, 0),
                Some(p) => assert!(b.continues(p), "seq {} after {}", b.seq, p.seq),
            }
            if b.iq.len() > 100 {
                let c = cycles_per_sample(&b.iq);
                assert!((c - 0.125).abs() < 1e-3, "{c}");
            }
            samples += b.iq.len();
            prev = Some(b);
        }

        tx.send(HwCmd::WfPlan {
            slot: 0,
            views: vec![],
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(100));
        while rx.try_recv().is_ok() {}
        assert!(
            rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "stopped"
        );

        tx.send(wf_start(8)).unwrap();
        let b = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!((b.generation, b.seq), (8, 0));
        stop.store(true, Ordering::Relaxed);
        t.join().unwrap().unwrap();
    }

    #[test]
    fn a_wide_continuous_view_is_read_at_the_capped_rate() {
        let (tx, stop, t) = start(Scene::new().tone(F0, -10.0));
        let (s, rx, _) = Subscriber::new(1024);
        tx.send(HwCmd::SubscribeWf(s)).unwrap();
        // decim 8: 15.36 MS/s
        tx.send(HwCmd::WfPlan {
            slot: 0,
            views: vec![WfView {
                hz: F0,
                decim: 8,
                generation: 1,
                fps: 20.0,
            }],
        })
        .unwrap();
        let t0 = Instant::now();
        let mut samples = 0;
        while t0.elapsed() < Duration::from_secs(2) {
            if let Ok(b) = rx.recv_timeout(Duration::from_millis(100)) {
                samples += b.iq.len();
            }
        }
        stop.store(true, Ordering::Relaxed);
        t.join().unwrap().unwrap();
        // (the sim makes WF samples as they're read; hardware and the iq
        // backend lose the rest in their rings, as gaps)
        let rate = samples as f64 / 2.0;
        assert!((1.0e6..=1.6e6).contains(&rate), "{rate} S/s read");
    }

    #[test]
    fn wf_time_shares_a_slot_between_views() {
        // two views of the same tone: 1.92 MHz span centred on F0 (tone at
        // +fs/8) and 480 kHz centred 60 kHz below the tone (+fs/8 again)
        let fs1 = ADC_HZ_HF / 64.0;
        let fs2 = ADC_HZ_HF / 256.0;
        let tone = F0 + fs1 / 8.0;
        let (tx, stop, t) = start(Scene::new().tone(tone, -10.0));
        let (s, rx, _) = Subscriber::new(256);
        tx.send(HwCmd::SubscribeWf(s)).unwrap();
        let v = |generation, hz, decim| WfView {
            hz,
            decim,
            generation,
            fps: 10.0,
        };
        tx.send(HwCmd::WfPlan {
            slot: 1,
            views: vec![v(1, F0, 64), v(2, tone - fs2 / 8.0, 256)],
        })
        .unwrap();
        // per view: samples per capture, captures
        let mut shot = [0usize; 3];
        let mut shots = [0usize; 3];
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_millis(1050) {
            let Ok(b) = rx.recv_timeout(Duration::from_millis(100)) else {
                continue;
            };
            assert_eq!(b.slot, 1);
            let g = b.generation as usize;
            if b.gap_before {
                if shots[g] > 0 {
                    assert_eq!(shot[g], WF_BLOCK, "a whole block per capture");
                }
                shots[g] += 1;
                shot[g] = 0;
            }
            shot[g] += b.iq.len();
            if b.iq.len() > 100 {
                let c = cycles_per_sample(&b.iq);
                assert!((c - 0.125).abs() < 2e-3, "view {g}: {c}");
            }
        }
        stop.store(true, Ordering::Relaxed);
        t.join().unwrap().unwrap();
        // 10 captures a second each (both spans fill a block in < 20 ms)
        for g in [1, 2] {
            assert!((10..=12).contains(&shots[g]), "{shots:?}");
        }
    }

    #[test]
    fn read_pause_waits_for_the_rest_of_a_batch() {
        let fps = 48_000.0; // 240 frames per 5 ms batch
        assert_eq!(read_pause(0, fps), RX_BATCH);
        let p = read_pause(60, fps).as_secs_f64();
        assert!((p - 0.00375).abs() < 1e-9, "{p}");
        assert_eq!(read_pause(240, fps), Duration::ZERO);
        assert_eq!(read_pause(2000, fps), Duration::ZERO);
    }

    #[test]
    fn rate_monitor_reports_deficit_and_restarts_on_rate_change() {
        let mut m = RateMonitor::new(Duration::from_secs(1));
        let t0 = Instant::now();
        let ms = |k: u64| t0 + Duration::from_millis(k);
        // 48 kS/s nominal (122.88 MHz / 256 / 10); deliver 47 000/s
        assert_eq!(m.add(ms(0), 0, 10, 122.88e6), None);
        for k in 1..10 {
            assert_eq!(m.add(ms(k * 100), 4700, 10, 122.88e6), None);
        }
        let r = m.add(ms(1000), 4700, 10, 122.88e6).unwrap();
        assert_eq!(
            r,
            RateReport {
                got: 47_000.0,
                want: 48_000.0
            }
        );
        assert!((r.deficit() - 1.0 / 48.0).abs() < 1e-9);
        // rate change: the window restarts, nothing reported yet
        assert_eq!(m.add(ms(1100), 4800, 20, 122.88e6), None);
        let r = m.add(ms(2100), 24_000, 20, 122.88e6).unwrap();
        assert!((r.got - 24_000.0).abs() < 1e-6 && (r.want - 24_000.0).abs() < 1e-6);
        assert!(r.deficit().abs() < 1e-9);
    }
}
