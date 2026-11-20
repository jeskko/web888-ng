// SPDX-License-Identifier: GPL-3.0-or-later
//! The receiver's time sources as a reference clock for the system
//! (`[timesync]`, `doc/api.md` §8.5).
//!
//! A source is anything that measures the system clock's error against a
//! time it receives: today the LF time-code stations ([`crate::tcmon`]),
//! later others (the EFR teleswitch telegrams, GPS). It calls [`submit`]
//! with a [`Sample`] per measurement: the offset (true time minus the
//! system's, its own calibration applied), when, and its own standard
//! error. Nothing else is needed to add one.
//!
//! Once a minute the hub combines the samples that came in since the
//! last time, one per source (a source's several averaged):
//!
//! * offsets of half a second or more (the clock far off, as after a
//!   start without network: no battery-backed clock) only when two
//!   sources agree within [`AGREE_S`]: a wrong decode doesn't set the
//!   clock;
//! * with two sources or more, one beyond [`OUTLIER_SIGMAS`] of its error
//!   (at least [`OUTLIER_MIN_S`]) from their median is left out;
//! * the rest weighted by their errors (inverse variance);
//!
//! and sends the result to chrony's SOCK reference clock (chrony.conf:
//! `refclock SOCK PATH`), stamped with the time it is sent at (the
//! offset moves by the clock's frequency error over the seconds since:
//! microseconds). Without chrony (no socket) the sends fail quietly and
//! are counted; the state is shown either way ([`snapshot`]).

use std::collections::BTreeMap;
use std::os::unix::net::UnixDatagram;
use std::sync::Mutex;
use std::time::Duration;
use web888_config::Timesync;

/// Agreement needed for a large offset (s).
pub const AGREE_S: f64 = 0.02;
/// Large: needs [`AGREE_S`] (s).
pub const LARGE_S: f64 = 0.5;
/// A source this many of its errors from the median is left out...
pub const OUTLIER_SIGMAS: f64 = 4.0;
/// ... but never for less than this (s).
pub const OUTLIER_MIN_S: f64 = 0.003;
/// chrony's sample: struct sock_sample (64-bit time_t), its magic.
const SOCK_MAGIC: i32 = 0x534f_434b;

/// One measurement of the system clock.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    /// The source, e.g. a station's name (`[timesync] trim_ms` keys).
    pub source: String,
    /// True time minus system time (s).
    pub offset: f64,
    /// When it held (system time, s).
    pub at: f64,
    /// Its standard error (s).
    pub sigma: f64,
}

/// A source's latest, for the metrics.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct SourceState {
    pub offset: Option<f64>,
    pub sigma: Option<f64>,
    /// Minutes it took part in, and was left out of.
    pub used: u64,
    pub rejected: u64,
}

/// What the hub did, for the metrics and the admin page.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct State {
    pub sources: BTreeMap<String, SourceState>,
    /// The last combined sample: offset, error (s), sources used.
    pub offset: Option<f64>,
    pub sigma: Option<f64>,
    pub n: usize,
    /// Samples sent to chrony; failed sends (no chrony); minutes with no
    /// usable sample (some came, none passed).
    pub sent: u64,
    pub send_errors: u64,
    pub rejected: u64,
}

struct Hub {
    cfg: Timesync,
    pending: Vec<Sample>,
    state: State,
}

static HUB: Mutex<Option<Hub>> = Mutex::new(None);

/// A source's measurement (dropped while the hub doesn't run).
pub fn submit(s: Sample) {
    if !s.offset.is_finite() || !s.sigma.is_finite() || s.sigma <= 0.0 {
        return;
    }
    if let Some(h) = HUB.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        h.pending.push(s);
    }
}

/// The hub's state (none: it doesn't run).
pub fn snapshot() -> Option<State> {
    HUB.lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|h| h.state.clone())
}

/// Start the hub: a thread combining and sending each minute.
pub fn spawn(cfg: Timesync) -> std::io::Result<()> {
    let path = cfg.chrony_socket.clone();
    *HUB.lock().unwrap_or_else(|e| e.into_inner()) = Some(Hub {
        cfg,
        pending: Vec::new(),
        state: State::default(),
    });
    if path.is_empty() {
        eprintln!("timesync: no chrony socket ([timesync] chrony_socket): measuring only");
    }
    std::thread::Builder::new()
        .name("timesync".into())
        .spawn(move || {
            let sock = UnixDatagram::unbound().ok();
            loop {
                // at each minute's 30th second: the time codes' minutes
                // (reported a few seconds after the minute) are in
                let now = now_s();
                let wait = 30.0 - now.rem_euclid(60.0);
                std::thread::sleep(Duration::from_secs_f64(wait.rem_euclid(60.0).max(1.0)));
                let mut g = HUB.lock().unwrap_or_else(|e| e.into_inner());
                let Some(h) = g.as_mut() else { return };
                let samples = std::mem::take(&mut h.pending);
                let Some((offset, _)) = h.combine(&samples) else {
                    continue;
                };
                if path.is_empty() {
                    continue;
                }
                let sent = sock
                    .as_ref()
                    .is_some_and(|s| s.send_to(&sock_sample(now_s(), offset), &path).is_ok());
                if sent {
                    h.state.sent += 1;
                } else {
                    h.state.send_errors += 1;
                }
            }
        })?;
    Ok(())
}

fn now_s() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// chrony's `struct sock_sample` with a 64-bit `time_t` (40 bytes; chrony
/// 4.8 takes the 32-bit layout too): the system time, the offset (true
/// minus system), no pulse, no leap second.
fn sock_sample(at: f64, offset: f64) -> [u8; 40] {
    let sec = at.floor();
    let usec = ((at - sec) * 1e6).floor().min(999_999.0);
    let mut b = [0u8; 40];
    b[0..8].copy_from_slice(&(sec as i64).to_ne_bytes());
    b[8..16].copy_from_slice(&(usec as i64).to_ne_bytes());
    b[16..24].copy_from_slice(&offset.to_ne_bytes());
    // pulse, leap, padding: 0
    b[36..40].copy_from_slice(&SOCK_MAGIC.to_ne_bytes());
    b
}

impl Hub {
    /// The minute's samples combined (offset, error), or none.
    fn combine(&mut self, samples: &[Sample]) -> Option<(f64, f64)> {
        // one per source: the mean, its error by the count
        let mut per: BTreeMap<&str, (f64, f64, usize)> = BTreeMap::new();
        for s in samples {
            let trim = self.cfg.trim_ms.get(&s.source).copied().unwrap_or(0.0) / 1e3;
            let e = per.entry(&s.source).or_insert((0.0, 0.0, 0));
            e.0 += s.offset + trim;
            e.1 += s.sigma;
            e.2 += 1;
        }
        let mut v: Vec<(String, f64, f64)> = per
            .into_iter()
            .map(|(k, (o, sg, n))| {
                (
                    k.to_string(),
                    o / n as f64,
                    sg / n as f64 / (n as f64).sqrt(),
                )
            })
            .collect();
        if v.is_empty() {
            return None;
        }
        for (k, o, sg) in &v {
            let st = self.state.sources.entry(k.clone()).or_default();
            st.offset = Some(*o);
            st.sigma = Some(*sg);
        }
        let keep = select(&v);
        for (i, (k, _, _)) in v.iter().enumerate() {
            let st = self.state.sources.entry(k.clone()).or_default();
            if keep.contains(&i) {
                st.used += 1;
            } else {
                st.rejected += 1;
            }
        }
        let kept: Vec<(String, f64, f64)> = keep.iter().map(|&i| v[i].clone()).collect();
        v = kept;
        if v.is_empty() {
            self.state.rejected += 1;
            return None;
        }
        let w: f64 = v.iter().map(|x| 1.0 / (x.2 * x.2)).sum();
        let o = v.iter().map(|x| x.1 / (x.2 * x.2)).sum::<f64>() / w;
        let sg = 1.0 / w.sqrt();
        self.state.offset = Some(o);
        self.state.sigma = Some(sg);
        self.state.n = v.len();
        Some((o, sg))
    }
}

/// The indices of the samples (source, offset, error) to use.
fn select(v: &[(String, f64, f64)]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..v.len()).collect();
    if v.len() >= 2 {
        let mut o: Vec<f64> = v.iter().map(|x| x.1).collect();
        o.sort_by(f64::total_cmp);
        let med = if o.len() % 2 == 1 {
            o[o.len() / 2]
        } else {
            (o[o.len() / 2 - 1] + o[o.len() / 2]) / 2.0
        };
        idx.retain(|&i| (v[i].1 - med).abs() <= (OUTLIER_SIGMAS * v[i].2).max(OUTLIER_MIN_S));
    }
    // a large offset: two sources agreeing
    let cand = idx.clone();
    idx.retain(|&i| {
        v[i].1.abs() < LARGE_S
            || cand
                .iter()
                .any(|&j| j != i && (v[j].1 - v[i].1).abs() <= AGREE_S)
    });
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hub() -> Hub {
        Hub {
            cfg: Timesync::default(),
            pending: Vec::new(),
            state: State::default(),
        }
    }

    fn s(src: &str, offset: f64, sigma: f64) -> Sample {
        Sample {
            source: src.into(),
            offset,
            at: 0.0,
            sigma,
        }
    }

    #[test]
    fn sources_weigh_by_their_errors() {
        let mut h = hub();
        let (o, sg) = h
            .combine(&[s("RBU", 0.001, 0.0003), s("DCF77", 0.004, 0.003)])
            .unwrap();
        // 100:1 weights
        assert!((o - (0.001 * 100.0 + 0.004) / 101.0).abs() < 1e-9, "{o}");
        assert!(sg < 0.0003);
        assert_eq!(h.state.n, 2);
    }

    #[test]
    fn an_outlier_is_left_out() {
        let mut h = hub();
        let (o, _) = h
            .combine(&[
                s("RBU", 0.001, 0.0003),
                s("MSF", 0.002, 0.002),
                s("DCF77", 0.060, 0.003),
            ])
            .unwrap();
        assert!((o - 0.001).abs() < 0.0003, "{o}");
        assert_eq!(h.state.sources["DCF77"].rejected, 1);
    }

    /// The clock 3 s off: one source alone doesn't move it, two agreeing
    /// do.
    #[test]
    fn a_large_offset_needs_two_sources() {
        let mut h = hub();
        assert_eq!(h.combine(&[s("RBU", 3.0, 0.0003)]), None);
        assert_eq!(h.state.rejected, 1);
        let (o, _) = h
            .combine(&[s("RBU", 3.0, 0.0003), s("MSF", 3.004, 0.002)])
            .unwrap();
        assert!((o - 3.0).abs() < 0.001);
        // a small one alone is fine
        assert!(h.combine(&[s("RBU", 0.002, 0.0003)]).is_some());
    }

    #[test]
    fn a_source_is_averaged_and_trimmed() {
        let mut h = hub();
        h.cfg.trim_ms.insert("DCF39".into(), 1.0);
        let (o, sg) = h
            .combine(&[s("DCF39", 0.001, 0.001), s("DCF39", 0.003, 0.001)])
            .unwrap();
        assert!((o - 0.003).abs() < 1e-12, "{o}");
        assert!((sg - 0.001 / 2f64.sqrt()).abs() < 1e-12);
    }

    #[test]
    fn the_sample_is_chronys_layout() {
        let b = sock_sample(1_795_000_000.25, -0.0015);
        assert_eq!(
            i64::from_ne_bytes(b[0..8].try_into().unwrap()),
            1_795_000_000
        );
        assert_eq!(i64::from_ne_bytes(b[8..16].try_into().unwrap()), 250_000);
        assert_eq!(f64::from_ne_bytes(b[16..24].try_into().unwrap()), -0.0015);
        assert_eq!(
            i32::from_ne_bytes(b[36..40].try_into().unwrap()),
            SOCK_MAGIC
        );
    }
}
