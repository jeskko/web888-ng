// SPDX-License-Identifier: GPL-3.0-or-later
//! Time-sharing one WF slot between views (`doc/m3-multi-user.md` §3).
//!
//! With more views than slots, a slot captures one-shot blocks for its
//! views in turn. Each view is due once per line period (1 / fps); the
//! slot serves the most overdue view, and idles when none is due, so an
//! underloaded slot gives every view its requested rate and no more (a
//! wide span fills a block in microseconds; capturing it thousands of
//! times a second would only cost memory bandwidth). Overloaded, every
//! view gets the same fraction of its requested rate, whatever its span:
//! a narrow span's block takes longer, so it takes more of the slot.

use std::time::{Duration, Instant};

/// One view of a WF slot's plan.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WfView {
    pub hz: f64,
    pub decim: u16,
    /// Identifies the view in its blocks (`WfBlock::generation`).
    pub generation: u64,
    /// Lines per second wanted: one block per line.
    pub fps: f64,
}

impl WfView {
    fn period(&self) -> Duration {
        Duration::from_secs_f64(1.0 / self.fps.clamp(0.1, 100.0))
    }
}

/// Which view a time-shared slot captures next.
#[derive(Debug, Default)]
pub struct Rotation {
    views: Vec<(WfView, Instant)>,
}

impl Rotation {
    /// Take a new set of views; views kept (same generation) keep their
    /// place, new ones are due with the most overdue (now, if none is).
    pub fn set(&mut self, views: &[WfView], now: Instant) {
        let old = std::mem::take(&mut self.views);
        let join = old
            .iter()
            .filter(|(o, _)| views.iter().any(|v| v.generation == o.generation))
            .map(|&(_, d)| d)
            .min()
            .map_or(now, |d| d.min(now));
        self.views = views
            .iter()
            .map(|v| {
                let due = old
                    .iter()
                    .find(|(o, _)| o.generation == v.generation)
                    .map_or(join, |&(_, d)| d);
                (*v, due)
            })
            .collect();
    }

    pub fn views(&self) -> impl Iterator<Item = &WfView> {
        self.views.iter().map(|(v, _)| v)
    }

    /// The view to capture at `now`, or when the next one is due.
    pub fn next(&mut self, now: Instant) -> Result<WfView, Instant> {
        let first = self.views.iter().map(|&(_, d)| d).min();
        let first = first.ok_or(now + Duration::from_secs(3600))?;
        // bound the lag (after an overload, views don't burst to catch up
        // more than MAX_LAG), moving all together to keep their shares
        if let Some(floor) = now.checked_sub(MAX_LAG)
            && first < floor
        {
            for (_, d) in &mut self.views {
                *d += floor - first;
            }
        }
        let (k, &(v, due)) = self
            .views
            .iter()
            .enumerate()
            .min_by_key(|(_, (_, due))| *due)
            .expect("not empty");
        if due > now {
            return Err(due);
        }
        self.views[k].1 = due + v.period();
        Ok(v)
    }
}

/// How far behind its schedule a time-shared slot may fall.
const MAX_LAG: Duration = Duration::from_secs(1);

#[cfg(test)]
mod tests {
    use super::*;

    const ADC: f64 = 122.88e6;
    const BLOCK: f64 = 8192.0;

    fn view(generation: u64, decim: u16, fps: f64) -> WfView {
        WfView {
            hz: 7e6,
            decim,
            generation,
            fps,
        }
    }

    /// Run a slot on a fake clock for `secs`: blocks captured per view.
    fn simulate(views: &[WfView], secs: f64) -> Vec<f64> {
        let t0 = Instant::now();
        let mut now = t0;
        let mut r = Rotation::default();
        r.set(views, now);
        let mut blocks = vec![0.0; views.len()];
        while now < t0 + Duration::from_secs_f64(secs) {
            match r.next(now) {
                Ok(v) => {
                    let k = views.iter().position(|x| x == &v).unwrap();
                    blocks[k] += 1.0;
                    // the capture takes the block's signal time
                    now += Duration::from_secs_f64(BLOCK * v.decim as f64 / ADC);
                }
                Err(due) => now = due,
            }
        }
        blocks.iter().map(|b| b / secs).collect()
    }

    #[test]
    fn underloaded_views_get_their_rate() {
        // three wide views: blocks take < 1 ms
        let v = [view(1, 2, 20.0), view(2, 16, 20.0), view(3, 64, 10.0)];
        let got = simulate(&v, 10.0);
        for (g, v) in got.iter().zip(&v) {
            assert!((g - v.fps).abs() <= 0.2, "{got:?}");
        }
    }

    #[test]
    fn overloaded_views_get_the_same_share() {
        // decim 8192: a block is 0.55 s; 1024: 68 ms; 2: 0.13 ms
        let v = [view(1, 8192, 20.0), view(2, 1024, 20.0), view(3, 2, 10.0)];
        let got = simulate(&v, 60.0);
        let share: Vec<f64> = got.iter().zip(&v).map(|(g, v)| g / v.fps).collect();
        // 0.55 + 0.068 + 0.5 * 0.13e-3 s per round: ~1.6 rounds/s
        for s in &share {
            assert!((s - share[0]).abs() < 0.05 * share[0], "{share:?}");
        }
        assert!((share[0] - 1.0 / 12.3).abs() < 0.01, "{share:?}");
    }

    #[test]
    fn a_new_plan_keeps_kept_views_in_place() {
        let t0 = Instant::now();
        let mut r = Rotation::default();
        r.set(&[view(1, 2, 10.0), view(2, 2, 10.0)], t0);
        assert_eq!(r.next(t0).unwrap().generation, 1);
        assert_eq!(r.next(t0).unwrap().generation, 2);
        // both served: nothing due until 100 ms later
        assert_eq!(r.next(t0), Err(t0 + Duration::from_millis(100)));
        // view 3 joins: due at once, the others keep their times
        r.set(&[view(2, 2, 10.0), view(3, 2, 10.0)], t0);
        assert_eq!(r.next(t0).unwrap().generation, 3);
        assert_eq!(r.next(t0), Err(t0 + Duration::from_millis(100)));
        let t1 = t0 + Duration::from_millis(100);
        assert_eq!(r.next(t1).unwrap().generation, 2);
    }
}
