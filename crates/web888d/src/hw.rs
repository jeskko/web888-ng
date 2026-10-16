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
            if got > 0 {
                frames += got as u64;
                let iq = buf[..got * n].iter().map(|s| (s.i, s.q)).collect();
                match self.out.try_send(Block {
                    nch: n,
                    iq,
                    overload: false,
                }) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => dropped += 1,
                    Err(TrySendError::Disconnected(_)) => return Ok(()),
                }
            } else {
                std::thread::sleep(Duration::from_millis(5));
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
