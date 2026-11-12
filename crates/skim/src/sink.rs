// SPDX-License-Identifier: GPL-3.0-or-later
//! How the log sinks (MQTT, syslog) are doing, for the metrics.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[derive(Default)]
pub struct SinkStats {
    /// A server is set.
    pub configured: AtomicBool,
    /// Connected (UDP syslog: the socket is open).
    pub connected: AtomicBool,
    /// Messages sent, and those dropped (no connection, or it failed).
    pub sent: AtomicU64,
    pub dropped: AtomicU64,
    /// Failed connects and sends.
    pub errors: AtomicU64,
}

impl SinkStats {
    pub const fn new() -> Self {
        SinkStats {
            configured: AtomicBool::new(false),
            connected: AtomicBool::new(false),
            sent: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            errors: AtomicU64::new(0),
        }
    }

    pub(crate) fn state(&self, configured: bool, connected: bool) {
        self.configured.store(configured, Ordering::Relaxed);
        self.connected.store(connected, Ordering::Relaxed);
    }

    pub(crate) fn count(&self, a: &AtomicU64, n: usize) {
        if n > 0 {
            a.fetch_add(n as u64, Ordering::Relaxed);
        }
    }
}

pub static MQTT: SinkStats = SinkStats::new();
pub static SYSLOG: SinkStats = SinkStats::new();
