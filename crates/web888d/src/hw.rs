// SPDX-License-Identifier: GPL-3.0-or-later
//! The hardware thread: sole owner of the `Radio`. Applies control commands,
//! reads RX frames and fans them out, and runs the PPS clock loop.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::time::{Duration, Instant};
use web888_hpsdr::server::{Block, Control};
use web888_radio::clock::AdcClock;
use web888_radio::*;

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

pub struct HwLoop {
    pub radio: Box<dyn Radio>,
    pub ctl: Receiver<Control>,
    pub out: SyncSender<Block>,
    pub stop: Arc<AtomicBool>,
    pub verbose: bool,
}

impl HwLoop {
    pub fn run(mut self) -> Result<()> {
        let caps = self.radio.caps();
        let n = caps.rx_channels;
        let mut tuned: Vec<Option<f64>> = vec![None; n];
        let mut clock = AdcClock::new(caps.adc_hz);
        let mut buf = vec![Iq32::default(); 512 * n];
        let mut dropped = 0u64;
        let mut frames = 0u64;
        let mut last_report = Instant::now();
        let mut fe = FrontEnd::default();
        let mut rate = RateMonitor::new(Duration::from_secs(10));
        let mut last_stats = self.radio.hw_stats()?;

        while !self.stop.load(Ordering::Relaxed) {
            while let Ok(c) = self.ctl.try_recv() {
                match c {
                    Control::Tune { rx, hz } if rx < n => {
                        self.radio.tune_rx(rx, hz)?;
                        tuned[rx] = Some(hz);
                    }
                    Control::Atten { db } => {
                        fe.atten_db = db.clamp(0.0, 31.5);
                        self.radio.front_end(fe)?;
                    }
                    _ => {}
                }
            }

            let got = self.radio.read_rx(&mut buf)?;
            if let Some(r) = rate.add(
                Instant::now(),
                got as u64,
                self.radio.rx_r2(),
                self.radio.adc_hz(),
            ) && (self.verbose || r.deficit() > 0.01)
            {
                eprintln!(
                    "rx: {:.0} frames/s, expected {:.0} ({:+.1} %){}",
                    r.got,
                    r.want,
                    -100.0 * r.deficit(),
                    if r.deficit() > 0.01 {
                        ": frames lost, the reader is falling behind"
                    } else {
                        ""
                    }
                );
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
            if got > 0 {
                frames += got as u64;
                let iq = buf[..got * n].iter().map(|s| (s.i, s.q)).collect();
                match self.out.try_send(Block {
                    nch: n,
                    iq,
                    overload,
                }) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => dropped += 1,
                    Err(TrySendError::Disconnected(_)) => return Ok(()),
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
                for (ch, f) in tuned.iter().enumerate() {
                    if let Some(f) = f {
                        self.radio.tune_rx(ch, *f)?;
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
                eprintln!("hw: {frames} frames, {dropped} blocks dropped");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
