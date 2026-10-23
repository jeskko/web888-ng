// SPDX-License-Identifier: GPL-3.0-or-later
//! The skimmers' log: what bands and uploaders do, for stderr and for
//! whoever subscribes (syslog, MQTT), and which of it a sink wants.

use std::sync::{Arc, OnceLock};
use tokio::sync::broadcast;
use web888_config::{LogEvent, SkimMode};

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

impl Event {
    pub fn kind(&self) -> LogEvent {
        match self.source {
            "skim" => LogEvent::Band,
            _ => LogEvent::Upload,
        }
    }
}

/// The spot's mode as it is named in spots ("FT8").
pub fn mode_name(m: SkimMode) -> &'static str {
    match m {
        SkimMode::Ft8 => "FT8",
        SkimMode::Ft4 => "FT4",
        SkimMode::Wspr => "WSPR",
    }
}

/// Does a sink that takes `events` (spots of `modes`, all if empty) want
/// an event of `kind` (a spot of `mode`)?
pub fn wants(events: &[LogEvent], modes: &[SkimMode], kind: LogEvent, mode: Option<&str>) -> bool {
    events.contains(&kind)
        && (kind != LogEvent::Spot
            || modes.is_empty()
            || mode.is_some_and(|m| modes.iter().any(|&x| mode_name(x) == m)))
}

/// The system's host name.
pub fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: gethostname writes at most `len` bytes into `buf`.
    let r = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if r != 0 {
        return "web888".into();
    }
    let n = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..n]).into_owned()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sinks_choose() {
        let all = [LogEvent::Spot, LogEvent::Band, LogEvent::Upload];
        assert!(wants(&all, &[], LogEvent::Spot, Some("FT8")));
        assert!(!wants(&all, &[SkimMode::Wspr], LogEvent::Spot, Some("FT8")));
        assert!(wants(&all, &[SkimMode::Wspr], LogEvent::Spot, Some("WSPR")));
        // the mode filter is for spots only
        assert!(wants(&all, &[SkimMode::Wspr], LogEvent::Band, None));
        assert!(!wants(&[LogEvent::Upload], &[], LogEvent::Band, None));
    }
}
