// SPDX-License-Identifier: GPL-3.0-or-later
//! Spots and the bus they go out on (`doc/m4-skimmers.md` §5).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

#[derive(Clone, Debug, PartialEq)]
pub struct Spot {
    /// Slot start, UTC seconds since the epoch.
    pub time: f64,
    pub mode: &'static str,
    pub dial_hz: f64,
    /// RF frequency of the signal (dial + audio offset of its lowest tone).
    pub freq_hz: f64,
    pub snr_db: f32,
    pub dt_s: f32,
    pub message: String,
    /// The transmitting station, when the message says.
    pub sender: Option<String>,
    pub grid: Option<String>,
    /// Received live (not from a recording or simulation).
    pub live: bool,
}

/// New spots for whoever listens, and the recent ones for late comers.
pub struct SpotBus {
    tx: broadcast::Sender<Arc<Spot>>,
    recent: Mutex<VecDeque<Arc<Spot>>>,
    keep: usize,
}

impl SpotBus {
    pub fn new(keep: usize) -> Arc<Self> {
        Arc::new(SpotBus {
            tx: broadcast::channel(1024).0,
            recent: Mutex::new(VecDeque::new()),
            keep,
        })
    }

    pub fn publish(&self, s: Spot) {
        let s = Arc::new(s);
        {
            let mut r = self.recent.lock().unwrap();
            if r.len() == self.keep {
                r.pop_front();
            }
            r.push_back(s.clone());
        }
        let _ = self.tx.send(s);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Spot>> {
        self.tx.subscribe()
    }

    /// Spots from slots at or after `since`, oldest first.
    pub fn recent(&self, since: f64) -> Vec<Arc<Spot>> {
        self.recent
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.time >= since)
            .cloned()
            .collect()
    }
}
