// SPDX-License-Identifier: GPL-3.0-or-later
//! Who is on the receiver (`doc/m3-multi-user.md` §2.3, §8): each session
//! publishes its name, dial, mode, channel and whether it shares them;
//! others see everyone in the user list, the sharing ones with their
//! frequency (and as markers), and a session waiting for a receiver can
//! listen along to one (following its publication).

use crate::proto::{Mode, UserMark};
use std::collections::BTreeMap;
use std::sync::Mutex;
use tokio::sync::watch;
use web888_core::wf::SessionId;

/// Longest name (callsign or nickname), characters.
pub const NAME_MAX: usize = 8;

/// A user's name: at most `NAME_MAX` printable characters, inline (so a
/// publication stays `Copy`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Name {
    len: u8,
    buf: [u8; 4 * NAME_MAX],
}

impl Name {
    /// Trimmed, control characters dropped, inner whitespace collapsed to
    /// one space, cut to `NAME_MAX` characters.
    pub fn new(s: &str) -> Self {
        let clean: String = s
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .filter(|c| !c.is_control())
            .take(NAME_MAX)
            .collect();
        let mut n = Name::default();
        n.buf[..clean.len()].copy_from_slice(clean.as_bytes());
        n.len = clean.len() as u8;
        n
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.buf[..self.len as usize]).unwrap_or("")
    }
}

/// What a session tells the others.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UserPub {
    pub name: Name,
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

    /// The other users, as `me` sees them: the sharing ones with their
    /// frequency and mode.
    pub fn marks(&self, me: SessionId) -> Vec<UserMark> {
        self.map
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _)| **id != me)
            .map(|(&id, rx)| {
                let p = *rx.borrow();
                UserMark {
                    id,
                    name: p.name.as_str().into(),
                    hz: p.share.then_some(p.hz),
                    mode: p.share.then_some(p.mode),
                    rx: p.channel.is_some(),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(hz: f64, channel: Option<usize>, share: bool) -> UserPub {
        UserPub {
            name: Name::new("N0CALL"),
            hz,
            mode: Mode::Usb,
            channel,
            share,
        }
    }

    #[test]
    fn names_are_cleaned_and_cut() {
        assert_eq!(Name::new("  OH2\tXYZ\n ").as_str(), "OH2 XYZ");
        assert_eq!(Name::new("abcdefghijk").as_str(), "abcdefgh");
        assert_eq!(Name::new("ÄÖÅäöåüß€").as_str(), "ÄÖÅäöåüß");
        assert_eq!(Name::new("a\u{7}b").as_str(), "ab");
        assert_eq!(Name::new("").as_str(), "");
    }

    #[test]
    fn marks_show_everyone_and_sharing_users_frequencies() {
        let u = Users::default();
        let _a = u.join(1, user(7.0e6, Some(0), true));
        let b = u.join(2, user(7.1e6, Some(1), false));
        let _c = u.join(3, user(7.2e6, None, true));
        let _d = u.join(4, user(7.3e6, Some(2), true));
        let ids = |v: Vec<UserMark>| v.iter().map(|m| m.id).collect::<Vec<_>>();
        let shown = |v: Vec<UserMark>| {
            v.iter()
                .filter(|m| m.hz.is_some())
                .map(|m| m.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(u.marks(4)), [1, 2, 3]);
        assert_eq!(shown(u.marks(9)), [1, 3, 4]);
        let m = u.marks(9);
        assert_eq!((m[0].name.as_str(), m[0].rx), ("N0CALL", true));
        assert_eq!((m[1].hz, m[1].mode), (None, None), "private");
        assert!(!m[2].rx, "waiting");
        b.send_modify(|p| p.share = true);
        assert_eq!(shown(u.marks(9)), [1, 2, 3, 4]);
        u.leave(1);
        assert_eq!(ids(u.marks(9)), [2, 3, 4]);
        assert!(u.follow(1).is_none());
        let f = u.follow(2).unwrap();
        assert_eq!(f.borrow().hz, 7.1e6);
    }
}
