// SPDX-License-Identifier: GPL-3.0-or-later
//! RX channel leases (`doc/m3-multi-user.md` §2): the only code that
//! decides which client gets which hardware RX channel.
//!
//! [`Manager`] is the policy, without threads or I/O: requests are granted
//! a free channel, or wait in a queue ordered by class (highest first) and
//! then by arrival. A class may have a quota of channels; a request over it
//! waits until its class holds fewer. With pre-emption on, a request that
//! finds no free channel takes one from the lowest class below its own
//! (the most recently granted there), and the evicted lease waits again at
//! the front of its class.
//!
//! [`Channels`] wraps it for the server: it retunes granted channels
//! through the hw thread, and publishes each lease's state to its holder
//! (a [`Lease`]), which can poll it (a protocol thread) or await changes
//! (a web session). Dropping a lease releases it.

use std::collections::{BTreeMap, HashMap};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use web888_hw::HwCmd;

/// Client classes, lowest first (design.md §6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    Web,
    /// HPSDR and other network protocols.
    Protocol,
    Skimmer,
    Admin,
}

impl Class {
    pub const ALL: [Class; 4] = [Class::Web, Class::Protocol, Class::Skimmer, Class::Admin];

    pub fn name(self) -> &'static str {
        match self {
            Class::Web => "web",
            Class::Protocol => "protocol",
            Class::Skimmer => "skimmer",
            Class::Admin => "admin",
        }
    }
}

pub type LeaseId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseState {
    /// Queued; `position` 1 is served next.
    Waiting {
        position: usize,
    },
    Granted {
        ch: usize,
    },
    /// Released (only seen by a holder whose manager is gone).
    Closed,
}

impl LeaseState {
    pub fn channel(self) -> Option<usize> {
        match self {
            LeaseState::Granted { ch } => Some(ch),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Policy {
    /// Higher classes may take channels from lower ones.
    pub preempt: bool,
    /// Most channels a class may hold (none: no limit).
    pub quota: HashMap<Class, usize>,
}

impl Policy {
    pub fn from_limits(l: &web888_config::Limits) -> Self {
        Policy {
            preempt: l.preempt,
            quota: HashMap::from([(Class::Protocol, l.hpsdr_channels)]),
        }
    }

    fn quota(&self, c: Class) -> usize {
        self.quota.get(&c).copied().unwrap_or(usize::MAX)
    }
}

/// A channel to retune: the hw thread's `Tune`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tune {
    pub ch: usize,
    pub hz: f64,
}

/// One lease, for listings (admin page).
#[derive(Clone, Debug, PartialEq)]
pub struct LeaseInfo {
    pub id: LeaseId,
    pub class: Class,
    pub label: String,
    pub hz: f64,
    pub state: LeaseState,
}

struct Entry {
    class: Class,
    label: String,
    hz: f64,
    /// Arrival order; an evicted lease keeps its own, so it waits in
    /// front of later arrivals of its class.
    requested: u64,
    /// When granted (order), for picking the newest as a victim.
    granted: Option<(usize, u64)>,
}

pub struct Manager {
    /// Holder of each hardware channel.
    owner: Vec<Option<LeaseId>>,
    leases: BTreeMap<LeaseId, Entry>,
    policy: Policy,
    next_id: LeaseId,
    clock: u64,
}

impl Manager {
    pub fn new(channels: usize, policy: Policy) -> Self {
        Manager {
            owner: vec![None; channels],
            leases: BTreeMap::new(),
            policy,
            next_id: 1,
            clock: 0,
        }
    }

    pub fn channels(&self) -> usize {
        self.owner.len()
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// Ask for a channel tuned to `hz`; it is granted at once if the
    /// policy allows (see [`Manager::state`]), else queued.
    pub fn acquire(&mut self, class: Class, label: &str, hz: f64) -> (LeaseId, Vec<Tune>) {
        let id = self.next_id;
        self.next_id += 1;
        let requested = self.tick();
        self.leases.insert(
            id,
            Entry {
                class,
                label: label.into(),
                hz,
                requested,
                granted: None,
            },
        );
        (id, self.schedule())
    }

    /// Give a lease up; its channel goes to the queue.
    pub fn release(&mut self, id: LeaseId) -> Vec<Tune> {
        let Some(e) = self.leases.remove(&id) else {
            return Vec::new();
        };
        if let Some((ch, _)) = e.granted {
            self.owner[ch] = None;
        }
        self.schedule()
    }

    /// Retune a lease: its channel at once if it has one.
    pub fn tune(&mut self, id: LeaseId, hz: f64) -> Option<Tune> {
        let e = self.leases.get_mut(&id)?;
        e.hz = hz;
        e.granted.map(|(ch, _)| Tune { ch, hz })
    }

    /// New limits apply to later grants; channels already held over a
    /// lowered quota are kept.
    pub fn set_policy(&mut self, p: Policy) -> Vec<Tune> {
        self.policy = p;
        self.schedule()
    }

    pub fn state(&self, id: LeaseId) -> Option<LeaseState> {
        let e = self.leases.get(&id)?;
        Some(match e.granted {
            Some((ch, _)) => LeaseState::Granted { ch },
            None => LeaseState::Waiting {
                position: 1 + self.queue().iter().position(|&q| q == id).unwrap_or(0),
            },
        })
    }

    pub fn leases(&self) -> Vec<LeaseInfo> {
        self.leases
            .iter()
            .map(|(&id, e)| LeaseInfo {
                id,
                class: e.class,
                label: e.label.clone(),
                hz: e.hz,
                state: self.state(id).expect("listed"),
            })
            .collect()
    }

    /// Waiting leases in service order.
    fn queue(&self) -> Vec<LeaseId> {
        let mut q: Vec<(&LeaseId, &Entry)> = self
            .leases
            .iter()
            .filter(|(_, e)| e.granted.is_none())
            .collect();
        q.sort_by_key(|(_, e)| (std::cmp::Reverse(e.class), e.requested));
        q.into_iter().map(|(&id, _)| id).collect()
    }

    fn held(&self, class: Class) -> usize {
        self.leases
            .values()
            .filter(|e| e.class == class && e.granted.is_some())
            .count()
    }

    fn grant(&mut self, id: LeaseId, ch: usize) -> Tune {
        let t = self.tick();
        self.owner[ch] = Some(id);
        let e = self.leases.get_mut(&id).expect("granted lease exists");
        e.granted = Some((ch, t));
        Tune { ch, hz: e.hz }
    }

    /// The channel to take for a request of `class`: the lowest class
    /// below it, most recently granted there.
    fn victim(&self, class: Class) -> Option<LeaseId> {
        self.leases
            .iter()
            .filter_map(|(&id, e)| Some((id, e.class, e.granted?.1)))
            .filter(|&(_, c, _)| c < class)
            .min_by_key(|&(_, c, t)| (c, std::cmp::Reverse(t)))
            .map(|(id, _, _)| id)
    }

    /// Serve the queue while channels (or, with pre-emption, victims) last.
    fn schedule(&mut self) -> Vec<Tune> {
        let mut tunes = Vec::new();
        'serve: loop {
            for id in self.queue() {
                let class = self.leases[&id].class;
                if self.held(class) >= self.policy.quota(class) {
                    continue; // waits for its own class to free one
                }
                if let Some(ch) = self.owner.iter().position(Option::is_none) {
                    tunes.push(self.grant(id, ch));
                    continue 'serve;
                }
                if self.policy.preempt
                    && let Some(v) = self.victim(class)
                {
                    let (ch, _) = self.leases.get_mut(&v).unwrap().granted.take().unwrap();
                    tunes.push(self.grant(id, ch));
                    continue 'serve;
                }
                // no channel for this one, so none for the lower ones after it
                break 'serve;
            }
            break;
        }
        tunes
    }
}

/// [`Manager`] for the server: shared, retunes granted channels through
/// the hw thread and tells lease holders about changes.
pub struct Channels {
    inner: Mutex<Inner>,
}

struct Inner {
    mgr: Manager,
    hw: Sender<HwCmd>,
    watchers: HashMap<LeaseId, watch::Sender<LeaseState>>,
}

impl Inner {
    /// Retune first, then tell the holders, so a holder that sees its
    /// grant gets blocks from the new tuning soon after.
    fn apply(&mut self, tunes: Vec<Tune>) {
        for t in tunes {
            let _ = self.hw.send(HwCmd::Tune { ch: t.ch, hz: t.hz });
        }
        for (id, tx) in &self.watchers {
            if let Some(s) = self.mgr.state(*id) {
                tx.send_if_modified(|v| {
                    let changed = *v != s;
                    *v = s;
                    changed
                });
            }
        }
    }
}

impl Channels {
    pub fn new(channels: usize, policy: Policy, hw: Sender<HwCmd>) -> Arc<Self> {
        Arc::new(Channels {
            inner: Mutex::new(Inner {
                mgr: Manager::new(channels, policy),
                hw,
                watchers: HashMap::new(),
            }),
        })
    }

    /// Ask for a channel; see the lease's state for the answer.
    pub fn acquire(self: &Arc<Self>, class: Class, label: &str, hz: f64) -> Lease {
        let mut g = self.inner.lock().unwrap();
        let (id, tunes) = g.mgr.acquire(class, label, hz);
        let (tx, mut rx) = watch::channel(LeaseState::Closed);
        g.watchers.insert(id, tx);
        g.apply(tunes);
        // the holder starts from this state: `changed` waits for the next
        rx.mark_unchanged();
        Lease {
            id,
            chans: self.clone(),
            rx,
        }
    }

    pub fn set_policy(&self, p: Policy) {
        let mut g = self.inner.lock().unwrap();
        if g.mgr.policy != p {
            let tunes = g.mgr.set_policy(p);
            g.apply(tunes);
        }
    }

    pub fn leases(&self) -> Vec<LeaseInfo> {
        self.inner.lock().unwrap().mgr.leases()
    }

    fn release(&self, id: LeaseId) {
        let mut g = self.inner.lock().unwrap();
        g.watchers.remove(&id);
        let tunes = g.mgr.release(id);
        g.apply(tunes);
    }

    fn tune(&self, id: LeaseId, hz: f64) {
        let g = &mut *self.inner.lock().unwrap();
        if let Some(t) = g.mgr.tune(id, hz) {
            let _ = g.hw.send(HwCmd::Tune { ch: t.ch, hz: t.hz });
        }
    }
}

/// A holder's claim on a channel; released when dropped.
pub struct Lease {
    id: LeaseId,
    chans: Arc<Channels>,
    rx: watch::Receiver<LeaseState>,
}

impl Lease {
    pub fn id(&self) -> LeaseId {
        self.id
    }

    pub fn state(&self) -> LeaseState {
        *self.rx.borrow()
    }

    /// The hardware channel now carrying this lease, if granted.
    pub fn channel(&self) -> Option<usize> {
        self.state().channel()
    }

    /// Where the channel should be (now, or when granted).
    pub fn tune(&self, hz: f64) {
        self.chans.tune(self.id, hz);
    }

    /// Wait for the next change of state (grant, revocation, queue
    /// position) and return it.
    pub async fn changed(&mut self) -> LeaseState {
        if self.rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
        *self.rx.borrow_and_update()
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.chans.release(self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use LeaseState::*;

    fn policy(preempt: bool, quotas: &[(Class, usize)]) -> Policy {
        Policy {
            preempt,
            quota: quotas.iter().copied().collect(),
        }
    }

    fn take(m: &mut Manager, class: Class, hz: f64) -> LeaseId {
        m.acquire(class, "t", hz).0
    }

    #[test]
    fn grants_free_channels_and_tunes_them() {
        let mut m = Manager::new(2, Policy::default());
        let (a, t) = m.acquire(Class::Web, "a", 7.0e6);
        assert_eq!(t, [Tune { ch: 0, hz: 7.0e6 }]);
        let (b, t) = m.acquire(Class::Web, "b", 14.0e6);
        assert_eq!(t, [Tune { ch: 1, hz: 14.0e6 }]);
        assert_eq!(m.state(a), Some(Granted { ch: 0 }));
        assert_eq!(m.state(b), Some(Granted { ch: 1 }));
        assert_eq!(m.tune(a, 7.1e6), Some(Tune { ch: 0, hz: 7.1e6 }));
    }

    #[test]
    fn queue_is_fifo_and_release_serves_it() {
        let mut m = Manager::new(1, Policy::default());
        let a = take(&mut m, Class::Web, 1.0e6);
        let b = take(&mut m, Class::Web, 2.0e6);
        let c = take(&mut m, Class::Web, 3.0e6);
        assert_eq!(m.state(b), Some(Waiting { position: 1 }));
        assert_eq!(m.state(c), Some(Waiting { position: 2 }));
        // a waiting lease keeps its tuning for the grant, no hw command now
        assert_eq!(m.tune(b, 2.5e6), None);
        assert_eq!(m.release(a), [Tune { ch: 0, hz: 2.5e6 }]);
        assert_eq!(m.state(b), Some(Granted { ch: 0 }));
        assert_eq!(m.state(c), Some(Waiting { position: 1 }));
        assert_eq!(m.state(a), None);
        // releasing a waiting lease frees nothing
        assert_eq!(m.release(c), []);
        assert_eq!(m.release(c), [], "twice is harmless");
    }

    #[test]
    fn higher_class_waits_in_front() {
        let mut m = Manager::new(1, Policy::default());
        let a = take(&mut m, Class::Web, 1.0);
        let w = take(&mut m, Class::Web, 2.0);
        let p = take(&mut m, Class::Protocol, 3.0);
        assert_eq!(m.state(p), Some(Waiting { position: 1 }));
        assert_eq!(m.state(w), Some(Waiting { position: 2 }));
        m.release(a);
        assert_eq!(m.state(p), Some(Granted { ch: 0 }));
    }

    #[test]
    fn quota_holds_a_class_back_but_not_others() {
        let mut m = Manager::new(3, policy(false, &[(Class::Protocol, 1)]));
        let p1 = take(&mut m, Class::Protocol, 1.0);
        let p2 = take(&mut m, Class::Protocol, 2.0);
        let w = take(&mut m, Class::Web, 3.0);
        assert_eq!(m.state(p1), Some(Granted { ch: 0 }));
        assert_eq!(m.state(p2), Some(Waiting { position: 1 }));
        // the quota-blocked request doesn't block the web user behind it
        assert_eq!(m.state(w), Some(Granted { ch: 1 }));
        // a freed web channel doesn't go to the over-quota request...
        let w2 = take(&mut m, Class::Web, 4.0);
        assert_eq!(m.state(w2), Some(Granted { ch: 2 }));
        m.release(w);
        assert_eq!(m.state(p2), Some(Waiting { position: 1 }));
        // ...but one of its own class does
        let t = m.release(p1);
        assert_eq!(t, [Tune { ch: 0, hz: 2.0 }]);
        assert_eq!(m.state(p2), Some(Granted { ch: 0 }));
    }

    #[test]
    fn raising_a_quota_serves_waiting_requests() {
        let mut m = Manager::new(3, policy(false, &[(Class::Protocol, 1)]));
        take(&mut m, Class::Protocol, 1.0);
        let p2 = take(&mut m, Class::Protocol, 2.0);
        assert_eq!(m.state(p2), Some(Waiting { position: 1 }));
        let t = m.set_policy(policy(false, &[(Class::Protocol, 2)]));
        assert_eq!(t, [Tune { ch: 1, hz: 2.0 }]);
        // lowering it keeps the channels already held
        assert_eq!(m.set_policy(policy(false, &[(Class::Protocol, 0)])), []);
        assert_eq!(m.state(p2), Some(Granted { ch: 1 }));
    }

    #[test]
    fn no_preemption_by_default() {
        let mut m = Manager::new(1, Policy::default());
        let w = take(&mut m, Class::Web, 1.0);
        let a = take(&mut m, Class::Admin, 2.0);
        assert_eq!(m.state(w), Some(Granted { ch: 0 }));
        assert_eq!(m.state(a), Some(Waiting { position: 1 }));
    }

    #[test]
    fn preemption_takes_the_newest_lowest_class_channel() {
        let mut m = Manager::new(3, policy(true, &[]));
        let w1 = take(&mut m, Class::Web, 1.0);
        let p = take(&mut m, Class::Protocol, 2.0);
        let w2 = take(&mut m, Class::Web, 3.0);
        let w3 = take(&mut m, Class::Web, 4.0); // waits
        assert_eq!(m.state(w3), Some(Waiting { position: 1 }));
        let (a, t) = m.acquire(Class::Admin, "admin", 5.0);
        // w2 is the newest web lease; the protocol one is a higher class
        assert_eq!(t, [Tune { ch: 2, hz: 5.0 }]);
        assert_eq!(m.state(a), Some(Granted { ch: 2 }));
        assert_eq!(m.state(w1), Some(Granted { ch: 0 }));
        assert_eq!(m.state(p), Some(Granted { ch: 1 }));
        // the evicted lease waits in front of the later arrival w3
        assert_eq!(m.state(w2), Some(Waiting { position: 1 }));
        assert_eq!(m.state(w3), Some(Waiting { position: 2 }));
        // and gets the next free channel, at its own tuning
        assert_eq!(m.release(w1), [Tune { ch: 0, hz: 3.0 }]);
        assert_eq!(m.state(w2), Some(Granted { ch: 0 }));
    }

    #[test]
    fn preemption_spares_equal_and_higher_classes() {
        let mut m = Manager::new(2, policy(true, &[]));
        take(&mut m, Class::Protocol, 1.0);
        take(&mut m, Class::Admin, 2.0);
        let p = take(&mut m, Class::Protocol, 3.0);
        assert_eq!(m.state(p), Some(Waiting { position: 1 }));
        let w = take(&mut m, Class::Web, 4.0);
        assert_eq!(m.state(w), Some(Waiting { position: 2 }));
    }

    #[test]
    fn preemption_respects_quotas() {
        let mut m = Manager::new(1, policy(true, &[(Class::Protocol, 0)]));
        let w = take(&mut m, Class::Web, 1.0);
        let p = take(&mut m, Class::Protocol, 2.0);
        assert_eq!(m.state(w), Some(Granted { ch: 0 }));
        assert_eq!(m.state(p), Some(Waiting { position: 1 }));
    }

    #[test]
    fn turning_preemption_on_serves_the_queue() {
        let mut m = Manager::new(1, Policy::default());
        let w = take(&mut m, Class::Web, 1.0);
        let p = take(&mut m, Class::Protocol, 2.0);
        assert_eq!(m.set_policy(policy(true, &[])), [Tune { ch: 0, hz: 2.0 }]);
        assert_eq!(m.state(p), Some(Granted { ch: 0 }));
        assert_eq!(m.state(w), Some(Waiting { position: 1 }));
    }

    #[test]
    fn listing() {
        let mut m = Manager::new(1, Policy::default());
        let a = m.acquire(Class::Web, "web 10.0.0.2", 7.0e6).0;
        let b = m.acquire(Class::Protocol, "hpsdr rx0", 3.5e6).0;
        let l = m.leases();
        assert_eq!(l.len(), 2);
        assert_eq!(
            l[0],
            LeaseInfo {
                id: a,
                class: Class::Web,
                label: "web 10.0.0.2".into(),
                hz: 7.0e6,
                state: Granted { ch: 0 },
            }
        );
        assert_eq!((l[1].id, l[1].state), (b, Waiting { position: 1 }));
    }

    #[tokio::test]
    async fn channels_retune_and_notify_holders() {
        let (tx, rx) = std::sync::mpsc::channel();
        let chans = Channels::new(1, Policy::default(), tx);
        let a = chans.acquire(Class::Web, "a", 7.0e6);
        let mut b = chans.acquire(Class::Web, "b", 14.0e6);
        assert_eq!(a.state(), Granted { ch: 0 });
        assert_eq!(b.state(), Waiting { position: 1 });
        let tunes: Vec<(usize, f64)> = rx
            .try_iter()
            .map(|c| match c {
                HwCmd::Tune { ch, hz } => (ch, hz),
                _ => panic!("tune expected"),
            })
            .collect();
        assert_eq!(tunes, [(0, 7.0e6)]);

        let waiter = tokio::spawn(async move {
            let s = b.changed().await;
            (s, b)
        });
        tokio::task::yield_now().await;
        drop(a);
        let (s, b) = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(s, Granted { ch: 0 });
        assert!(matches!(rx.try_recv(), Ok(HwCmd::Tune { ch: 0, hz }) if hz == 14.0e6));
        b.tune(14.1e6);
        assert!(matches!(rx.try_recv(), Ok(HwCmd::Tune { ch: 0, hz }) if hz == 14.1e6));
        assert_eq!(chans.leases().len(), 1);
        drop(b);
        assert!(chans.leases().is_empty());
    }
}
