// SPDX-License-Identifier: GPL-3.0-or-later
//! The sessions' waterfalls (`doc/m3-multi-user.md` §3): the scheduler's
//! hardware views, the slot plans sent to the hw thread, and a
//! [`ViewJob`] on the DSP pool per view, which sessions join and leave.
//! (Views a session draws from its own RX channel are its `AudioJob`'s.)

use crate::workers::{ViewCmd, ViewJob, ViewSub, WfSink};
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use web888_core::pool::DspPool;
use web888_core::wf::{HwReq, SessionId, ViewInfo, WfScheduler};
use web888_hw::HwCmd;

/// A session's hardware view request.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HwView {
    pub center_hz: f64,
    pub decim: u16,
    pub bins: usize,
    pub fps: f64,
}

pub struct Waterfalls {
    inner: Mutex<Inner>,
    pool: Arc<DspPool>,
    adc_hz: f64,
}

struct Inner {
    sched: WfScheduler,
    hw: Sender<HwCmd>,
    /// Each view's job (by generation).
    jobs: HashMap<u64, crossbeam_channel::Sender<ViewCmd>>,
    /// Each session's view.
    sessions: HashMap<SessionId, u64>,
}

impl Waterfalls {
    pub fn new(
        slots: usize,
        adc_hz: f64,
        block: usize,
        hw: Sender<HwCmd>,
        pool: Arc<DspPool>,
    ) -> Self {
        Waterfalls {
            inner: Mutex::new(Inner {
                sched: WfScheduler::new(slots, adc_hz, block, 1),
                hw,
                jobs: HashMap::new(),
                sessions: HashMap::new(),
            }),
            pool,
            adc_hz,
        }
    }

    /// Show session `s` the hardware view `v` (`None`: no hardware view,
    /// e.g. it draws from its channel), at most `budget` FFTs a line.
    pub fn set(&self, s: SessionId, v: Option<HwView>, sink: &WfSink, budget: usize) {
        let g = &mut *self.inner.lock().unwrap();
        let old = g.sessions.get(&s).copied();
        let new = g.sched.set(
            s,
            v.map(|v| HwReq {
                hz: v.center_hz,
                decim: v.decim,
                fps: v.fps,
            }),
        );
        for (slot, views) in g.sched.plans() {
            let _ = g.hw.send(HwCmd::WfPlan { slot, views });
        }
        // views no session watches end with their jobs
        let live: Vec<u64> = g.sched.views().iter().map(|v| v.generation).collect();
        g.jobs.retain(|k, _| live.contains(k));
        if let Some(o) = old
            && Some(o) != new
            && let Some(tx) = g.jobs.get(&o)
        {
            let _ = tx.send(ViewCmd::Remove(s));
        }
        match (new, v) {
            (Some(n), Some(v)) => {
                let tx = g.jobs.entry(n).or_insert_with(|| {
                    let (tx, rx) = crossbeam_channel::unbounded();
                    self.pool.add(Box::new(ViewJob::new(
                        n,
                        v.center_hz,
                        self.adc_hz,
                        v.decim,
                        budget,
                        rx,
                    )));
                    tx
                });
                let _ = tx.send(ViewCmd::Add(ViewSub {
                    session: s,
                    bins: v.bins,
                    fps: v.fps,
                    sink: sink.clone(),
                    new_gen: old != new,
                }));
                g.sessions.insert(s, n);
            }
            _ => {
                g.sessions.remove(&s);
            }
        }
    }

    /// The session is gone.
    pub fn remove(&self, s: SessionId) {
        // the sink isn't used without a view
        let g = &mut *self.inner.lock().unwrap();
        let old = g.sessions.remove(&s);
        g.sched.set(s, None);
        for (slot, views) in g.sched.plans() {
            let _ = g.hw.send(HwCmd::WfPlan { slot, views });
        }
        let live: Vec<u64> = g.sched.views().iter().map(|v| v.generation).collect();
        g.jobs.retain(|k, _| live.contains(k));
        if let Some(o) = old
            && let Some(tx) = g.jobs.get(&o)
        {
            let _ = tx.send(ViewCmd::Remove(s));
        }
    }

    pub fn views(&self) -> Vec<ViewInfo> {
        self.inner.lock().unwrap().sched.views()
    }
}
