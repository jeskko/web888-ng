// SPDX-License-Identifier: GPL-3.0-or-later
//! Passwords and logins (`doc/m3-multi-user.md` §7).
//!
//! Two passwords, both argon2id hashes in the config: the admin's and an
//! optional shared listener password (none: the receiver is open). A login
//! gets a random 256-bit token in an HttpOnly, SameSite=Strict cookie,
//! kept server side for 12 h. A login stays valid only while the password
//! it was checked against is unchanged, so changing a password logs out
//! everyone who used the old one.
//!
//! Failed logins are limited per client address (they age out; a success
//! doesn't clear them, or the listener password would reset the count
//! between admin guesses), and hashes are checked one at a time (argon2 takes about a second on the A9 by design).

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use axum::http::HeaderMap;
use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use web888_config::Auth;

pub const COOKIE: &str = "web888d";
pub const LOGIN_TTL: Duration = Duration::from_secs(12 * 3600);
/// Failed logins allowed per address within `FAIL_WINDOW`.
pub const FAIL_LIMIT: usize = 5;
pub const FAIL_WINDOW: Duration = Duration::from_secs(60);
/// Shortest admin password accepted.
pub const ADMIN_MIN_LEN: usize = 8;
const MAX_LOGINS: usize = 1000;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Listener,
    Admin,
}

/// An argon2id PHC string for `password` (random salt).
pub fn hash(password: &str) -> String {
    Argon2::default()
        .hash_password(password.as_bytes())
        .expect("argon2 with a random salt")
        .to_string()
}

/// Does `password` match the PHC string `hash`? (A malformed hash
/// matches nothing.)
pub fn verify(hash: &str, password: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|h| {
        Argon2::default()
            .verify_password(password.as_bytes(), &h)
            .is_ok()
    })
}

/// The role a password gives under `auth`, and the hash it matched.
/// The admin password also lets one listen.
pub fn check(auth: &Auth, password: &str, want: Role) -> Option<(Role, String)> {
    if let Some(h) = &auth.admin_hash
        && verify(h, password)
    {
        return Some((Role::Admin, h.clone()));
    }
    match (&auth.listener_hash, want) {
        (Some(h), Role::Listener) if verify(h, password) => Some((Role::Listener, h.clone())),
        _ => None,
    }
}

struct Login {
    role: Role,
    /// The hash the password matched.
    hash: String,
    expires: Instant,
}

/// Logins and the failed-login record.
#[derive(Default)]
pub struct Logins {
    map: Mutex<HashMap<String, Login>>,
    fails: Mutex<HashMap<IpAddr, VecDeque<Instant>>>,
    /// One hash check at a time.
    pub checking: tokio::sync::Mutex<()>,
}

impl Logins {
    /// A new login; returns its token.
    pub fn add(&self, role: Role, hash: String) -> String {
        let mut b = [0u8; 32];
        getrandom::fill(&mut b).expect("system random source");
        let token: String = b.iter().map(|x| format!("{x:02x}")).collect();
        let now = Instant::now();
        let mut m = self.map.lock().unwrap();
        m.retain(|_, l| l.expires > now);
        if m.len() >= MAX_LOGINS {
            // the oldest goes
            if let Some(k) = m
                .iter()
                .min_by_key(|(_, l)| l.expires)
                .map(|(k, _)| k.clone())
            {
                m.remove(&k);
            }
        }
        m.insert(
            token.clone(),
            Login {
                role,
                hash,
                expires: now + LOGIN_TTL,
            },
        );
        token
    }

    pub fn remove(&self, token: &str) {
        self.map.lock().unwrap().remove(token);
    }

    /// The role of the request's login, if it is valid under `auth`.
    pub fn role(&self, headers: &HeaderMap, auth: &Auth) -> Option<Role> {
        let token = cookie(headers)?;
        let m = self.map.lock().unwrap();
        let l = m.get(token)?;
        let current = match l.role {
            Role::Admin => auth.admin_hash.as_ref(),
            Role::Listener => auth.listener_hash.as_ref(),
        };
        (l.expires > Instant::now() && current == Some(&l.hash)).then_some(l.role)
    }

    /// None if `ip` may try a password now, else how long until it may.
    pub fn blocked(&self, ip: IpAddr) -> Option<Duration> {
        let now = Instant::now();
        let mut f = self.fails.lock().unwrap();
        let q = f.get_mut(&ip)?;
        while q.front().is_some_and(|t| now - *t >= FAIL_WINDOW) {
            q.pop_front();
        }
        if q.is_empty() {
            f.remove(&ip);
            return None;
        }
        (q.len() >= FAIL_LIMIT).then(|| FAIL_WINDOW - (now - q[0]))
    }

    pub fn failed(&self, ip: IpAddr) {
        let mut f = self.fails.lock().unwrap();
        let now = Instant::now();
        // forget addresses that have been quiet
        if f.len() > 10_000 {
            f.retain(|_, q| q.back().is_some_and(|t| now - *t < FAIL_WINDOW));
        }
        f.entry(ip).or_default().push_back(now);
    }
}

/// The login token from the request's cookies.
pub fn cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v)
}

/// `Set-Cookie` for a login (`token`), or clearing it (`None`).
pub fn set_cookie(token: Option<&str>, secure: bool) -> String {
    let secure = if secure { "; Secure" } else { "" };
    match token {
        Some(t) => format!(
            "{COOKIE}={t}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{secure}",
            LOGIN_TTL.as_secs()
        ),
        None => format!("{COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{secure}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_verify_and_roles_follow_the_config() {
        let a = hash("correct horse");
        assert!(a.starts_with("$argon2id$"));
        assert!(verify(&a, "correct horse"));
        assert!(!verify(&a, "correct horsE"));
        assert!(!verify("not a hash", "x"));
        let l = hash("radio");
        let auth = Auth {
            admin_hash: Some(a.clone()),
            listener_hash: Some(l.clone()),
        };
        assert_eq!(
            check(&auth, "correct horse", Role::Listener),
            Some((Role::Admin, a.clone()))
        );
        assert_eq!(
            check(&auth, "radio", Role::Listener),
            Some((Role::Listener, l.clone()))
        );
        assert_eq!(check(&auth, "radio", Role::Admin), None);

        // a login lasts while its password does
        let logins = Logins::default();
        let t = logins.add(Role::Listener, l);
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::COOKIE,
            format!("x=1; {COOKIE}={t}").parse().unwrap(),
        );
        assert_eq!(logins.role(&h, &auth), Some(Role::Listener));
        let changed = Auth {
            listener_hash: Some(hash("other")),
            ..auth.clone()
        };
        assert_eq!(logins.role(&h, &changed), None);
        logins.remove(&t);
        assert_eq!(logins.role(&h, &auth), None);
    }

    #[test]
    fn failures_block_an_address_for_a_while() {
        let l = Logins::default();
        let ip: IpAddr = "192.0.2.1".parse().unwrap();
        let other: IpAddr = "192.0.2.2".parse().unwrap();
        for _ in 0..FAIL_LIMIT - 1 {
            l.failed(ip);
        }
        assert_eq!(l.blocked(ip), None);
        l.failed(ip);
        let d = l.blocked(ip).unwrap();
        assert!(d > FAIL_WINDOW - Duration::from_secs(1) && d <= FAIL_WINDOW);
        assert_eq!(l.blocked(other), None);
    }
}
