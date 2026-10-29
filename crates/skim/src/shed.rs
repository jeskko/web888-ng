// SPDX-License-Identifier: GPL-3.0-or-later
//! Decoder load shedding: when the decoders keep running out of time
//! (slots cut short or dropped: the CPU went to the listeners), the band
//! with the fewest recent decodes stops being decoded, one at a time;
//! after a calm spell the last one shed comes back. The bands' channels
//! keep running, so a band resumes at its next slot.

use crate::Band;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// Weight of each slot in the pressure (cut or dropped: 1, else 0).
const ALPHA: f64 = 0.2;
/// Pressure above which a band is shed, and below which one may resume.
const SHED_ABOVE: f64 = 0.5;
const CALM_BELOW: f64 = 0.1;
/// Least time between two sheds (two FT8 slots: the last one's effect).
const SHED_GAP: Duration = Duration::from_secs(30);
/// Calm time before a band resumes; doubled (to `HOLD_MAX`) when a band
/// has to be shed again soon after one resumed.
const HOLD: Duration = Duration::from_secs(120);
const HOLD_MAX: Duration = Duration::from_secs(1800);
const RELAPSE: Duration = Duration::from_secs(300);

/// What a decision did.
#[derive(Debug, PartialEq)]
pub enum Change {
    Shed(String),
    Resumed(String),
}

pub struct Shedder {
    pressure: f64,
    /// The bands running (the service keeps it current).
    pub bands: Vec<Arc<Band>>,
    /// Bands shed, the last at the end.
    shed: Vec<Arc<Band>>,
    last_change: Option<Instant>,
    last_resume: Option<Instant>,
    /// Since when the pressure has been below `CALM_BELOW`.
    calm_since: Option<Instant>,
    hold: Duration,
}

impl Default for Shedder {
    fn default() -> Self {
        Shedder {
            pressure: 0.0,
            bands: Vec::new(),
            shed: Vec::new(),
            last_change: None,
            last_resume: None,
            calm_since: None,
            hold: HOLD,
        }
    }
}

impl Shedder {
    /// A slot was decoded (`cut`: short of time) or dropped (`cut`).
    pub fn observe(&mut self, cut: bool, now: Instant) -> Option<Change> {
        self.shed
            .retain(|b| self.bands.iter().any(|x| Arc::ptr_eq(x, b)));
        let x = if cut { 1.0 } else { 0.0 };
        self.pressure += ALPHA * (x - self.pressure);
        if self.pressure >= CALM_BELOW {
            self.calm_since = None;
        } else if self.calm_since.is_none() {
            self.calm_since = Some(now);
        }
        let since = |t: Option<Instant>| t.map_or(Duration::MAX, |t| now.duration_since(t));
        if self.pressure > SHED_ABOVE && since(self.last_change) >= SHED_GAP {
            let active: Vec<&Arc<Band>> = self
                .bands
                .iter()
                .filter(|b| !b.status.shed.load(Ordering::Relaxed))
                .filter(|b| b.status.granted.load(Ordering::Relaxed))
                .collect();
            if active.len() < 2 {
                return None;
            }
            let b = active
                .into_iter()
                .min_by(|a, b| a.status.recent().total_cmp(&b.status.recent()))
                .expect("two or more")
                .clone();
            if since(self.last_resume) < RELAPSE {
                self.hold = (self.hold * 2).min(HOLD_MAX);
            }
            b.status.shed.store(true, Ordering::Relaxed);
            self.last_change = Some(now);
            // the next shed on fresh evidence
            self.pressure = SHED_ABOVE / 2.0;
            let label = b.label.clone();
            self.shed.push(b);
            return Some(Change::Shed(label));
        }
        if since(self.calm_since) >= self.hold && self.calm_since.is_some() {
            let b = self.shed.pop()?;
            b.status.shed.store(false, Ordering::Relaxed);
            if since(self.last_resume) > HOLD_MAX {
                self.hold = HOLD;
            }
            self.last_change = Some(now);
            self.last_resume = Some(now);
            // the next one after another calm spell
            self.calm_since = Some(now);
            return Some(Change::Resumed(b.label.clone()));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BandStatus;
    use web888_config::Skimmer;

    fn band(label: &str, recent: f64) -> Arc<Band> {
        let b = Arc::new(Band {
            cfg: Skimmer::default(),
            label: label.into(),
            status: BandStatus::default(),
        });
        b.status.granted.store(true, Ordering::Relaxed);
        b.status.set_recent(recent);
        b
    }

    #[test]
    fn the_quietest_band_goes_first_and_comes_back_last() {
        let (a, q, z) = (band("40m", 30.0), band("10m", 0.0), band("20m", 5.0));
        let mut s = Shedder {
            bands: vec![a.clone(), q.clone(), z.clone()],
            ..Shedder::default()
        };
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        // calm: nothing
        assert_eq!(s.observe(false, at(0)), None);
        // slots cut: the quietest band goes
        let mut got = None;
        for k in 0..10 {
            got = got.or(s.observe(true, at(k)));
        }
        assert_eq!(got, Some(Change::Shed("10m".into())));
        assert!(q.status.shed.load(Ordering::Relaxed));
        // still cut, but not before the gap
        assert_eq!(s.observe(true, at(20)), None);
        let mut got = None;
        for k in 40..50 {
            got = got.or(s.observe(true, at(k)));
        }
        assert_eq!(got, Some(Change::Shed("20m".into())));
        // never the last one
        for k in 100..120 {
            assert_eq!(s.observe(true, at(k)), None);
        }
        // calm (the pressure below 0.1 from 210 s): after the hold, the
        // last shed first, the next after another hold
        let mut back = Vec::new();
        for k in 200..600 {
            if let Some(c) = s.observe(false, at(k)) {
                back.push((k, c));
            }
        }
        assert_eq!(
            back,
            vec![
                (330, Change::Resumed("20m".into())),
                (450, Change::Resumed("10m".into()))
            ]
        );
    }

    #[test]
    fn a_relapse_doubles_the_hold() {
        let (a, q, z) = (band("40m", 30.0), band("10m", 0.0), band("20m", 5.0));
        let mut s = Shedder {
            bands: vec![a, q, z],
            ..Shedder::default()
        };
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut changes = Vec::new();
        // cut, calm (10m back), cut again soon after: 10m shed again
        for k in 0..200 {
            let cut = k < 20 || (160..180).contains(&k);
            if let Some(c) = s.observe(cut, at(k)) {
                changes.push(c);
            }
        }
        let shed = |b: &str| Change::Shed(b.into());
        assert_eq!(
            changes,
            vec![shed("10m"), Change::Resumed("10m".into()), shed("10m")]
        );
        assert_eq!(s.hold, 2 * HOLD);
    }
}
