// SPDX-License-Identifier: GPL-3.0-or-later
//! The waterfall scheduler (`doc/m3-multi-user.md` §3): which hardware
//! view each WF slot captures.
//!
//! Sessions asking for the same view (centre, decimation) share it: one
//! capture, one FFT engine, at the highest rate any of them asked for.
//! With no more views than slots, each view has a slot to itself and
//! streams continuously; with more, views are spread over the slots by
//! load (rate x time to fill a block, so a narrow span counts more) and
//! each slot captures its views in turn (`web888_hw::wf::Rotation`).
//!
//! Views a session computes from its own RX channel never come here.

use std::collections::BTreeMap;
use web888_hw::WfView;

pub type SessionId = u64;

/// A session's hardware view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HwReq {
    pub hz: f64,
    pub decim: u16,
    pub fps: f64,
}

/// One shared view, for listings (admin page, fairness).
#[derive(Clone, Debug, PartialEq)]
pub struct ViewInfo {
    pub generation: u64,
    pub hz: f64,
    pub decim: u16,
    pub fps: f64,
    pub sessions: Vec<SessionId>,
    pub slot: usize,
    /// The slot is shared with other views.
    pub shared: bool,
}

struct View {
    hz: f64,
    decim: u16,
    generation: u64,
    subs: BTreeMap<SessionId, f64>,
    slot: Option<usize>,
}

impl View {
    fn key(&self) -> (i64, u16) {
        (self.hz.round() as i64, self.decim)
    }

    fn fps(&self) -> f64 {
        self.subs.values().copied().fold(0.0, f64::max)
    }

    fn hw(&self) -> WfView {
        WfView {
            hz: self.hz,
            decim: self.decim,
            generation: self.generation,
            fps: self.fps(),
        }
    }
}

pub struct WfScheduler {
    slots: usize,
    /// Signal time to fill a one-shot block at decimation 1.
    block_secs: f64,
    views: Vec<View>,
    next_gen: u64,
    /// What each slot was last told.
    sent: Vec<Vec<WfView>>,
}

impl WfScheduler {
    /// `slots` WF slots capturing `block`-sample one-shot blocks at ADC
    /// rate `adc_hz`; generations start at `first_gen`.
    pub fn new(slots: usize, adc_hz: f64, block: usize, first_gen: u64) -> Self {
        WfScheduler {
            slots,
            block_secs: block as f64 / adc_hz,
            views: Vec::new(),
            next_gen: first_gen,
            sent: vec![Vec::new(); slots],
        }
    }

    /// Set (or with `None` clear) session `s`'s hardware view. Returns the
    /// generation its WF blocks will carry.
    pub fn set(&mut self, s: SessionId, req: Option<HwReq>) -> Option<u64> {
        let key = req.map(|r| (r.hz.round() as i64, r.decim));
        // the same view again: only its rate may change
        if let Some(r) = req
            && let Some(v) = self
                .views
                .iter_mut()
                .find(|v| Some(v.key()) == key && v.subs.contains_key(&s))
        {
            v.subs.insert(s, r.fps);
            return Some(v.generation);
        }
        for v in &mut self.views {
            v.subs.remove(&s);
        }
        self.views.retain(|v| !v.subs.is_empty());
        let r = req?;
        let key = (r.hz.round() as i64, r.decim);
        if let Some(v) = self.views.iter_mut().find(|v| v.key() == key) {
            v.subs.insert(s, r.fps);
            return Some(v.generation);
        }
        let generation = self.next_gen;
        self.next_gen += 1;
        self.views.push(View {
            hz: r.hz,
            decim: r.decim,
            generation,
            subs: BTreeMap::from([(s, r.fps)]),
            slot: None,
        });
        Some(generation)
    }

    /// Slot time a view needs (fraction of one slot).
    fn demand(&self, v: &View) -> f64 {
        v.fps() * self.block_secs * v.decim as f64
    }

    fn assign(&mut self) {
        if self.slots == 0 {
            return;
        }
        if self.views.len() <= self.slots {
            // a slot each; views keep the slot they had where they can
            let mut taken = vec![false; self.slots];
            for v in &mut self.views {
                if let Some(k) = v.slot
                    && !taken[k]
                {
                    taken[k] = true;
                } else {
                    v.slot = None;
                }
            }
            for v in &mut self.views {
                if v.slot.is_none() {
                    let k = taken.iter().position(|t| !t).expect("enough slots");
                    taken[k] = true;
                    v.slot = Some(k);
                }
            }
            return;
        }
        // greedy: heaviest first, each to the least loaded slot
        let mut order: Vec<usize> = (0..self.views.len()).collect();
        order.sort_by(|&a, &b| {
            self.demand(&self.views[b])
                .total_cmp(&self.demand(&self.views[a]))
                .then(self.views[a].generation.cmp(&self.views[b].generation))
        });
        let mut load = vec![0.0f64; self.slots];
        for i in order {
            let k = (0..self.slots)
                .min_by(|&a, &b| load[a].total_cmp(&load[b]))
                .expect("slots");
            load[k] += self.demand(&self.views[i]);
            self.views[i].slot = Some(k);
        }
    }

    /// The slot plans that changed since the last call (send each as
    /// `HwCmd::WfPlan`).
    pub fn plans(&mut self) -> Vec<(usize, Vec<WfView>)> {
        self.assign();
        let mut changed = Vec::new();
        for k in 0..self.slots {
            let mut p: Vec<WfView> = self
                .views
                .iter()
                .filter(|v| v.slot == Some(k))
                .map(View::hw)
                .collect();
            p.sort_by_key(|v| v.generation);
            if p != self.sent[k] {
                self.sent[k] = p.clone();
                changed.push((k, p));
            }
        }
        changed
    }

    pub fn views(&self) -> Vec<ViewInfo> {
        self.views
            .iter()
            .map(|v| {
                let slot = v.slot.unwrap_or(0);
                ViewInfo {
                    generation: v.generation,
                    hz: v.hz,
                    decim: v.decim,
                    fps: v.fps(),
                    sessions: v.subs.keys().copied().collect(),
                    slot,
                    shared: self.views.iter().filter(|o| o.slot == Some(slot)).count() > 1,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADC: f64 = 122.88e6;

    fn req(hz: f64, decim: u16, fps: f64) -> Option<HwReq> {
        Some(HwReq { hz, decim, fps })
    }

    fn gens(p: &[WfView]) -> Vec<u64> {
        p.iter().map(|v| v.generation).collect()
    }

    #[test]
    fn same_view_is_shared_at_the_highest_rate() {
        let mut s = WfScheduler::new(2, ADC, 8192, 1);
        let a = s.set(1, req(30.72e6, 2, 20.0)).unwrap();
        let b = s.set(2, req(30.72e6, 2, 10.0)).unwrap();
        assert_eq!(a, b);
        let p = s.plans();
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].0, p[0].1.len(), p[0].1[0].fps), (0, 1, 20.0));
        // the faster one leaves: the plan follows the slower one's rate
        s.set(1, None);
        assert_eq!(s.plans()[0].1[0].fps, 10.0);
        // the last one leaves: the slot stops
        s.set(2, None);
        assert_eq!(s.plans(), [(0, vec![])]);
        assert!(s.plans().is_empty(), "no change, nothing to send");
    }

    #[test]
    fn views_get_their_own_slots_then_share() {
        let mut s = WfScheduler::new(2, ADC, 8192, 1);
        let a = s.set(1, req(30.72e6, 2, 20.0)).unwrap();
        let b = s.set(2, req(7.1e6, 256, 20.0)).unwrap();
        let mut p = s.plans();
        p.sort_by_key(|x| x.0);
        assert_eq!(
            p.iter().map(|x| gens(&x.1)).collect::<Vec<_>>(),
            [vec![a], vec![b]]
        );
        // a third view: the light ones share a slot, the heaviest keeps one
        let c = s.set(3, req(14.1e6, 8192, 20.0)).unwrap();
        let mut plan = [vec![], vec![]];
        for (k, v) in s.plans() {
            plan[k] = gens(&v);
        }
        plan.sort();
        assert_eq!(plan, [vec![a, b], vec![c]]);
        assert!(
            s.views()
                .iter()
                .find(|v| v.generation == c)
                .is_some_and(|v| !v.shared)
        );
        // back to two: each streams on its own again
        s.set(3, None);
        for (_, v) in s.plans() {
            assert!(v.len() <= 1);
        }
        let v = s.views();
        assert_eq!(v.len(), 2);
        assert_ne!(v[0].slot, v[1].slot);
    }

    #[test]
    fn a_view_keeps_its_slot_when_another_leaves() {
        let mut s = WfScheduler::new(2, ADC, 8192, 1);
        s.set(1, req(1e6, 2, 20.0));
        let b = s.set(2, req(2e6, 2, 20.0)).unwrap();
        s.plans();
        s.set(1, None);
        // slot 0 stops; b stays on slot 1, not restarted
        assert_eq!(s.plans(), [(0, vec![])]);
        assert_eq!(s.views()[0].generation, b);
        assert_eq!(s.views()[0].slot, 1);
    }

    #[test]
    fn asking_again_keeps_the_view() {
        let mut s = WfScheduler::new(2, ADC, 8192, 1);
        let a = s.set(1, req(7e6, 64, 20.0)).unwrap();
        s.plans();
        assert_eq!(s.set(1, req(7e6, 64, 10.0)), Some(a));
        let p = s.plans();
        assert_eq!(gens(&p[0].1), [a]);
        assert_eq!(p[0].1[0].fps, 10.0);
    }

    #[test]
    fn moving_a_view_is_a_new_one() {
        let mut s = WfScheduler::new(2, ADC, 8192, 1);
        let a = s.set(1, req(7e6, 64, 20.0)).unwrap();
        let b = s.set(1, req(7.1e6, 64, 20.0)).unwrap();
        assert_ne!(a, b);
        let p = s.plans();
        assert_eq!(p.len(), 1);
        assert_eq!(gens(&p[0].1), [b]);
        assert_eq!(s.views().len(), 1);
    }
}
