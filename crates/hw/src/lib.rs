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

/// Commands for the hw thread, from any consumer (HPSDR, web sessions).
pub enum HwCmd {
    Tune {
        ch: usize,
        hz: f64,
    },
    Atten {
        db: f32,
    },
    /// Stream WF slot `slot` continuously; its blocks carry `generation`.
    WfStart {
        slot: usize,
        hz: f64,
        decim: u16,
        generation: u64,
    },
    WfStop {
        slot: usize,
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
            },
            rx,
            dropped,
        )
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
            Ok(()) => true,
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

/// A WF slot being streamed.
struct WfRun {
    hz: f64,
    decim: u16,
    generation: u64,
    seq: u64,
    /// `Radio::wf_gaps` already accounted for.
    gaps_seen: u64,
    /// Mark the next block as following a gap (restart after a retune).
    force_gap: bool,
}

/// WF samples per read; a few reads per loop pass at most, so a fast
/// waterfall can't starve RX.
const WF_READ: usize = 16384;
const WF_READS_PER_PASS: usize = 8;

impl HwLoop {
    pub fn run(mut self) -> Result<()> {
        let caps = self.radio.caps();
        let n = caps.rx_channels;
        let mut tuned: Vec<Option<f64>> = vec![None; n];
        let mut wf: Vec<Option<WfRun>> = (0..caps.wf_channels).map(|_| None).collect();
        let mut rx_out = Fanout::<RxBlock>::default();
        let mut wf_out = Fanout::<WfBlock>::default();
        let mut clock = AdcClock::new(caps.adc_hz);
        let mut buf = vec![Iq32::default(); 512 * n];
        let mut wf_buf = vec![Iq16::default(); WF_READ];
        let mut frames = 0u64;
        let mut last_report = Instant::now();
        let mut fe = FrontEnd::default();
        let mut rate = RateMonitor::new(Duration::from_secs(10));
        let mut last_stats = self.radio.hw_stats()?;
        self.status.rx_rate_ok.store(true, Ordering::Relaxed);

        while !self.stop.load(Ordering::Relaxed) {
            while let Ok(c) = self.ctl.try_recv() {
                let res = match c {
                    HwCmd::Tune { ch, hz } => self.radio.tune_rx(ch, hz).map(|()| {
                        tuned[ch] = Some(hz);
                    }),
                    HwCmd::Atten { db } => {
                        fe.atten_db = db.clamp(0.0, 31.5);
                        self.radio.front_end(fe)
                    }
                    HwCmd::WfStart {
                        slot,
                        hz,
                        decim,
                        generation,
                    } => self
                        .radio
                        .start_wf(slot, hz, decim, WfMode::Continuous)
                        .map(|()| {
                            wf[slot] = Some(WfRun {
                                hz,
                                decim,
                                generation,
                                seq: 0,
                                gaps_seen: 0,
                                force_gap: false,
                            });
                        }),
                    HwCmd::WfStop { slot } if slot < wf.len() => {
                        // a one-shot block ends the stream on every backend
                        let r = match &wf[slot] {
                            Some(w) => self.radio.start_wf(slot, w.hz, w.decim, WfMode::OneShot),
                            None => Ok(()),
                        };
                        wf[slot] = None;
                        r
                    }
                    HwCmd::WfStop { .. } => Err(Error::InvalidChannel(usize::MAX)),
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

            for (slot, run) in wf.iter_mut().enumerate() {
                let Some(w) = run else { continue };
                for _ in 0..WF_READS_PER_PASS {
                    // gaps counted by now lie before this read's samples
                    let gaps = self.radio.wf_gaps(slot);
                    let got = self.radio.read_wf(slot, &mut wf_buf)?;
                    if got == 0 {
                        break;
                    }
                    let gap_before = std::mem::take(&mut w.force_gap) || gaps != w.gaps_seen;
                    w.gaps_seen = gaps;
                    wf_out.send(WfBlock {
                        slot,
                        generation: w.generation,
                        seq: w.seq,
                        gap_before,
                        iq: wf_buf[..got].to_vec(),
                    });
                    w.seq += 1;
                    if got < wf_buf.len() {
                        break;
                    }
                }
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
                for (slot, run) in wf.iter_mut().enumerate() {
                    if let Some(w) = run {
                        self.radio
                            .start_wf(slot, w.hz, w.decim, WfMode::Continuous)?;
                        w.gaps_seen = 0;
                        w.force_gap = true;
                    }
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
        // decim 64: fs 1.92 MHz; the tone sits fs/8 above the centre
        let fs = ADC_HZ_HF / 64.0;
        let (tx, stop, t) = start(Scene::new().tone(F0 + fs / 8.0, -10.0));
        let (s, rx, _) = Subscriber::new(256);
        tx.send(HwCmd::SubscribeWf(s)).unwrap();
        let wf_start = |generation| HwCmd::WfStart {
            slot: 0,
            hz: F0,
            decim: 64,
            generation,
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

        tx.send(HwCmd::WfStop { slot: 0 }).unwrap();
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
