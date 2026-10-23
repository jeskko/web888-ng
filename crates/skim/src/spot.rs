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
    /// WSPR: frequency drift over the transmission (Hz) and the
    /// transmitter's power (dBm).
    pub drift: Option<f32>,
    pub dbm: Option<i32>,
    /// Received live (not from a recording or simulation).
    pub live: bool,
}

impl Spot {
    /// One line for logs: mode, dial, slot time, SNR, DT, audio
    /// frequency, message.
    pub fn log_line(&self) -> String {
        let t = self.time as u64 % 86_400;
        format!(
            "{} {:.1} kHz {:02}{:02}{:02} {:3.0} {:4.1} {:4.0} {}",
            self.mode,
            self.dial_hz / 1e3,
            t / 3600,
            t / 60 % 60,
            t % 60,
            self.snr_db,
            self.dt_s,
            self.freq_hz - self.dial_hz,
            self.message
        )
    }
}

/// The amateur band of a frequency ("40m"), or "" outside them.
pub fn band_name(hz: f64) -> &'static str {
    const BANDS: &[(f64, f64, &str)] = &[
        (0.1357e6, 0.1378e6, "2200m"),
        (0.472e6, 0.479e6, "630m"),
        (1.8e6, 2.0e6, "160m"),
        (3.5e6, 4.0e6, "80m"),
        (5.06e6, 5.45e6, "60m"),
        (7.0e6, 7.3e6, "40m"),
        (10.1e6, 10.15e6, "30m"),
        (14.0e6, 14.35e6, "20m"),
        (18.068e6, 18.168e6, "17m"),
        (21.0e6, 21.45e6, "15m"),
        (24.89e6, 24.99e6, "12m"),
        (28.0e6, 29.7e6, "10m"),
        (50.0e6, 54.0e6, "6m"),
    ];
    BANDS
        .iter()
        .find(|b| (b.0..=b.1).contains(&hz))
        .map_or("", |b| b.2)
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
        // sent under the lock, so `follow` sees each spot exactly once
        let mut r = self.recent.lock().unwrap();
        if r.len() == self.keep {
            r.pop_front();
        }
        r.push_back(s.clone());
        let _ = self.tx.send(s);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Spot>> {
        self.tx.subscribe()
    }

    /// The recent spots from `since` on, and a receiver for the spots
    /// after those (none lost, none twice).
    pub fn follow(&self, since: f64) -> (Vec<Arc<Spot>>, broadcast::Receiver<Arc<Spot>>) {
        let r = self.recent.lock().unwrap();
        let old = r.iter().filter(|s| s.time >= since).cloned().collect();
        (old, self.tx.subscribe())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn spot(time: f64) -> Spot {
        Spot {
            time,
            mode: "FT8",
            dial_hz: 7_074_000.0,
            freq_hz: 7_075_000.0,
            snr_db: -10.0,
            dt_s: 0.1,
            message: "CQ K1ABC FN42".into(),
            sender: Some("K1ABC".into()),
            grid: Some("FN42".into()),
            drift: None,
            dbm: None,
            live: true,
        }
    }

    #[test]
    fn follow_gets_recent_then_new() {
        let bus = SpotBus::new(3);
        for t in [0.0, 15.0, 30.0, 45.0] {
            bus.publish(spot(t));
        }
        let (old, mut rx) = bus.follow(15.0);
        // kept 3, from 15 s on
        assert_eq!(
            old.iter().map(|s| s.time).collect::<Vec<_>>(),
            [15.0, 30.0, 45.0]
        );
        assert!(rx.try_recv().is_err());
        bus.publish(spot(60.0));
        assert_eq!(rx.try_recv().unwrap().time, 60.0);
        assert_eq!(bus.recent(50.0).len(), 1);
    }

    #[test]
    fn bands() {
        assert_eq!(band_name(7_074_000.0), "40m");
        assert_eq!(band_name(10_136_000.0), "30m");
        assert_eq!(band_name(14_074_000.0), "20m");
        assert_eq!(band_name(9_000_000.0), "");
    }
}
