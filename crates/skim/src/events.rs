// SPDX-License-Identifier: GPL-3.0-or-later
//! The skimmers' log: what bands and uploaders do, for stderr and for
//! whoever subscribes (syslog).

use std::sync::{Arc, OnceLock};
use tokio::sync::broadcast;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Notice,
    Warning,
}

#[derive(Clone, Debug)]
pub struct Event {
    /// UTC seconds.
    pub time: f64,
    pub level: Level,
    /// "skim", "pskreporter", "wsprnet".
    pub source: &'static str,
    pub text: String,
}

fn bus() -> &'static broadcast::Sender<Arc<Event>> {
    static BUS: OnceLock<broadcast::Sender<Arc<Event>>> = OnceLock::new();
    BUS.get_or_init(|| broadcast::channel(256).0)
}

/// Log `text` from `source` (stderr as `source: text`, and the bus).
pub fn emit(level: Level, source: &'static str, text: impl Into<String>) {
    let text = text.into();
    eprintln!("{source}: {text}");
    let _ = bus().send(Arc::new(Event {
        time: crate::unix_now(),
        level,
        source,
        text,
    }));
}

pub fn subscribe() -> broadcast::Receiver<Arc<Event>> {
    bus().subscribe()
}
