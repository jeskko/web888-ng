// SPDX-License-Identifier: GPL-3.0-or-later
//! Who is on the receiver (`doc/m3-multi-user.md` §2.3, §8): each session
//! publishes its dial, mode, channel and whether it shares them; others
//! see the sharing ones as markers, and a session waiting for a receiver
//! can listen along to one (following its publication).

use crate::proto::{Mode, UserMark};
use std::collections::BTreeMap;
use std::sync::Mutex;
use tokio::sync::watch;
use web888_core::wf::SessionId;

/// What a session tells the others.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UserPub {
    pub hz: f64,
    pub mode: Mode,
    /// Its own receiver's channel (none while it waits).
    pub channel: Option<usize>,
    pub share: bool,
}

impl UserPub {
    /// Others may see it and listen along.
    pub fn followable(&self) -> bool {
        self.share && self.channel.is_some()
    }
}

#[derive(Default)]
pub struct Users {
    map: Mutex<BTreeMap<SessionId, watch::Receiver<UserPub>>>,
}

impl Users {
    /// Register session `id`; it publishes through the returned sender.
    pub fn join(&self, id: SessionId, p: UserPub) -> watch::Sender<UserPub> {
        let (tx, rx) = watch::channel(p);
        self.map.lock().unwrap().insert(id, rx);
        tx
    }

    pub fn leave(&self, id: SessionId) {
        self.map.lock().unwrap().remove(&id);
    }

    /// Session `id`'s current publication.
    pub fn get(&self, id: SessionId) -> Option<UserPub> {
        self.map.lock().unwrap().get(&id).map(|rx| *rx.borrow())
    }

    /// Follow session `id`'s publication.
    pub fn follow(&self, id: SessionId) -> Option<watch::Receiver<UserPub>> {
        let mut rx = self.map.lock().unwrap().get(&id)?.clone();
        rx.mark_unchanged();
        Some(rx)
    }

    /// The users `me` may see: sharing, with a receiver.
    pub fn marks(&self, me: SessionId) -> Vec<UserMark> {
        self.map
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _)| **id != me)
            .filter_map(|(&id, rx)| {
                let p = *rx.borrow();
                p.followable().then_some(UserMark {
                    id,
                    hz: p.hz,
                    mode: p.mode,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(hz: f64, channel: Option<usize>, share: bool) -> UserPub {
        UserPub {
            hz,
            mode: Mode::Usb,
            channel,
            share,
        }
    }

    #[test]
    fn marks_show_sharing_users_with_a_receiver() {
        let u = Users::default();
        let _a = u.join(1, user(7.0e6, Some(0), true));
        let b = u.join(2, user(7.1e6, Some(1), false));
        let _c = u.join(3, user(7.2e6, None, true));
        let _d = u.join(4, user(7.3e6, Some(2), true));
        let ids = |v: Vec<UserMark>| v.iter().map(|m| m.id).collect::<Vec<_>>();
        assert_eq!(ids(u.marks(4)), [1]);
        assert_eq!(ids(u.marks(9)), [1, 4]);
        b.send_modify(|p| p.share = true);
        assert_eq!(ids(u.marks(9)), [1, 2, 4]);
        u.leave(1);
        assert_eq!(ids(u.marks(9)), [2, 4]);
        assert!(u.follow(1).is_none());
        let f = u.follow(2).unwrap();
        assert_eq!(f.borrow().hz, 7.1e6);
    }
}
