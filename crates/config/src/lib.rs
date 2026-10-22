// SPDX-License-Identifier: GPL-3.0-or-later
//! web888d configuration (`doc/m3-multi-user.md` §6).
//!
//! One TOML file; every `serve` option has a key here, and options given on
//! the command line override the file. A missing file means all defaults.
//! Saving is atomic (temp file, fsync, rename), so a power cut leaves the
//! old or the new file, never a torn one. [`Config::changes`] tells which
//! changed settings apply at once and which need a restart.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;

pub const DEFAULT_PATH: &str = "/etc/web888d/web888d.toml";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// Simulated radio (real time).
    Sim,
    /// Kernel driver `/dev/zynqsdr` (stock or open).
    #[default]
    Zynqsdr,
    /// Wideband IQ from an SDR or a recording, channelized like the FPGA.
    Iq,
}

impl FromStr for Backend {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "sim" => Ok(Backend::Sim),
            "zynqsdr" => Ok(Backend::Zynqsdr),
            "iq" => Ok(Backend::Iq),
            _ => Err(format!("unknown backend {s:?} (sim, zynqsdr, iq)")),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// The radio hardware (changes need a restart).
    pub radio: Radio,
    /// HPSDR Protocol 1 server (changes need a restart).
    pub hpsdr: Hpsdr,
    /// Web UI server.
    pub web: Web,
    /// Sessions and channel sharing (apply at once).
    pub limits: Limits,
    /// Password hashes (`web888d passwd`).
    pub auth: Auth,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Radio {
    /// Where the samples come from.
    pub backend: Backend,
    /// zynqsdr: a path, `auto` (the image's or the stock one) or `none`.
    pub bitstream: String,
    /// EXPERIMENTAL (test unit only): reprogram the Si5351 ADC clock (Hz).
    pub si5351_adc: Option<f64>,
    /// Backend sim: the simulated scene.
    pub sim: Sim,
    /// Backend iq: the wideband IQ input.
    pub iq: Iq,
}

impl Default for Radio {
    fn default() -> Self {
        Radio {
            backend: Backend::default(),
            bitstream: "auto".into(),
            si5351_adc: None,
            sim: Sim::default(),
            iq: Iq::default(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Sim {
    /// Tones at the antenna: [frequency Hz, level dBFS].
    pub tones: Vec<[f64; 2]>,
    /// Add the typical ADC noise floor.
    pub noise: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Iq {
    /// `file:PATH` (.cs8/.cs16/.cf32) or `soapy:ARGS`.
    pub source: Option<String>,
    /// Input centre frequency (Hz).
    pub freq: Option<f64>,
    /// Input sample rate: 122.88 MHz / 8, 16, ..., 256.
    pub rate: f64,
    /// Device gain elements (soapy), dB.
    pub gains: BTreeMap<String, f64>,
    /// Device analog bandwidth (Hz, 0 = the driver's choice).
    pub bandwidth: f64,
    /// Device antenna port (soapy).
    pub antenna: Option<String>,
    /// Start a file again at its end.
    #[serde(rename = "loop")]
    pub loop_file: bool,
}

impl Default for Iq {
    fn default() -> Self {
        Iq {
            source: None,
            freq: None,
            rate: 7.68e6,
            gains: BTreeMap::new(),
            bandwidth: 0.0,
            antenna: None,
            loop_file: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Hpsdr {
    pub enabled: bool,
    /// UDP listen address.
    pub listen: String,
    /// Board id reported to clients (1 = Hermes, 5 = Orion, 6 = Hermes-Lite).
    pub board: u8,
    /// Receivers offered (max 7).
    pub receivers: usize,
}

impl Default for Hpsdr {
    fn default() -> Self {
        Hpsdr {
            enabled: true,
            listen: "0.0.0.0:1024".into(),
            board: 1,
            receivers: 7,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum TlsMode {
    /// A self-signed certificate, kept in `state_dir`.
    #[default]
    SelfSigned,
    /// `cert` and `key` (PEM).
    Files,
    /// Plain HTTP only (behind a TLS-terminating proxy).
    Off,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Web {
    pub enabled: bool,
    /// HTTPS and plain HTTP on one port.
    pub listen: String,
    /// The web UI's files.
    pub ui_dir: PathBuf,
    pub tls: TlsMode,
    /// tls = files: the certificate (PEM).
    pub tls_cert: Option<PathBuf>,
    /// tls = files: the private key (PEM).
    pub tls_key: Option<PathBuf>,
    /// Where web888d keeps state (the self-signed certificate).
    pub state_dir: PathBuf,
    /// Initial dial frequency (Hz; default 7.074 MHz if in range).
    pub default_freq: Option<f64>,
    /// Most FFTs averaged per waterfall line (CPU budget; default 30, 6 on
    /// 32-bit ARM, where an FFT costs ~100x more: `doc/m3-multi-user.md` §4).
    pub wf_budget: usize,
    /// Serve Prometheus metrics at `/metrics` (no login).
    pub metrics: bool,
    /// Reverse proxies (addresses or networks, e.g. "10.0.0.5",
    /// "192.168.0.0/16") whose X-Forwarded-For and X-Forwarded-Proto are
    /// believed: for requests from them, the client is the rightmost
    /// forwarded address that isn't a trusted proxy itself.
    #[schemars(with = "Vec<String>")]
    pub trusted_proxies: Vec<IpNet>,
}

impl Default for Web {
    fn default() -> Self {
        Web {
            enabled: true,
            listen: "0.0.0.0:8073".into(),
            ui_dir: "/usr/share/web888d/ui".into(),
            tls: TlsMode::default(),
            tls_cert: None,
            tls_key: None,
            state_dir: "/var/lib/web888d".into(),
            default_freq: None,
            wf_budget: if cfg!(target_arch = "arm") { 6 } else { 30 },
            metrics: false,
            trusted_proxies: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    /// Sessions per client address (0 = no limit).
    pub per_ip: usize,
    /// Web session time limit, minutes (0 = none).
    pub session_minutes: u32,
    /// Idle (no input, hidden tab) limit, minutes (0 = none).
    pub idle_minutes: u32,
    /// Higher classes may take channels from lower ones.
    pub preempt: bool,
    /// Most RX channels HPSDR clients may hold.
    pub hpsdr_channels: usize,
    /// Show users' frequencies to others and allow listen-along, unless
    /// the user opts out.
    pub share_default: bool,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            per_ip: 4,
            session_minutes: 0,
            idle_minutes: 0,
            preempt: false,
            hpsdr_channels: 7,
            share_default: true,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Auth {
    /// argon2 hash of the admin password (`web888d passwd`); none = no
    /// admin login possible.
    pub admin_hash: Option<String>,
    /// argon2 hash of the shared listener password; none = open receiver.
    pub listener_hash: Option<String>,
}

/// Where `path` leads through symlinks (itself if it isn't one).
fn link_target(path: &Path) -> PathBuf {
    let mut p = path.to_path_buf();
    for _ in 0..16 {
        let Ok(t) = std::fs::read_link(&p) else { break };
        p = match p.parent() {
            Some(d) if t.is_relative() => d.join(t),
            _ => t,
        };
    }
    p
}

/// An address or a network: `192.0.2.1`, `10.0.0.0/8`, `fd00::/8`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IpNet {
    pub addr: IpAddr,
    pub prefix: u8,
}

impl IpNet {
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr.to_canonical(), ip.to_canonical()) {
            (IpAddr::V4(n), IpAddr::V4(a)) => {
                let mask = u32::MAX.checked_shl(32 - self.prefix as u32).unwrap_or(0);
                u32::from(n) & mask == u32::from(a) & mask
            }
            (IpAddr::V6(n), IpAddr::V6(a)) => {
                let mask = u128::MAX.checked_shl(128 - self.prefix as u32).unwrap_or(0);
                u128::from(n) & mask == u128::from(a) & mask
            }
            _ => false,
        }
    }
}

impl FromStr for IpNet {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let (a, p) = match s.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (s, None),
        };
        let addr: IpAddr = a
            .trim()
            .parse()
            .map_err(|_| format!("{s:?}: not an IP address or network"))?;
        let addr = addr.to_canonical();
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match p {
            Some(p) => p
                .trim()
                .parse::<u8>()
                .ok()
                .filter(|&p| p <= max)
                .ok_or(format!("{s:?}: bad prefix length"))?,
            None => max,
        };
        Ok(IpNet { addr, prefix })
    }
}

impl std::fmt::Display for IpNet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let max = if self.addr.is_ipv4() { 32 } else { 128 };
        if self.prefix == max {
            write!(f, "{}", self.addr)
        } else {
            write!(f, "{}/{}", self.addr, self.prefix)
        }
    }
}

impl Serialize for IpNet {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for IpNet {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// What a config change means for a running server.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Settings that take effect at once changed.
    pub live: bool,
    /// Settings that need a restart changed (their section names).
    pub restart: Vec<&'static str>,
}

#[derive(Debug)]
pub enum Error {
    Io(PathBuf, std::io::Error),
    Parse(PathBuf, toml::de::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(p, e) => write!(f, "{}: {e}", p.display()),
            Error::Parse(p, e) => write!(f, "{}: {e}", p.display()),
        }
    }
}

impl std::error::Error for Error {}

impl Config {
    /// JSON Schema of the file (the admin page builds its form from it).
    pub fn schema() -> schemars::Schema {
        schemars::schema_for!(Config)
    }

    /// Read `path`; a missing file gives the defaults.
    pub fn load(path: &Path) -> Result<Self, Error> {
        match std::fs::read_to_string(path) {
            Ok(s) => Self::parse(&s).map_err(|e| Error::Parse(path.into(), e)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(Error::Io(path.into(), e)),
        }
    }

    pub fn parse(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("config serialises")
    }

    /// Write atomically: a temp file next to `path`, fsync, rename, fsync
    /// the directory. A symlink is followed and its target replaced (the
    /// image links /etc/web888d/web888d.toml to the config partition).
    pub fn save(&self, path: &Path) -> Result<(), Error> {
        let io = |e| Error::Io(path.into(), e);
        let path = &link_target(path);
        let dir = path
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir).map_err(io)?;
        let tmp = dir.join(format!(
            ".{}.tmp",
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("config")
        ));
        let mut f = std::fs::File::create(&tmp).map_err(io)?;
        f.write_all(self.to_toml().as_bytes()).map_err(io)?;
        f.sync_all().map_err(io)?;
        drop(f);
        std::fs::rename(&tmp, path).map_err(io)?;
        std::fs::File::open(dir)
            .and_then(|d| d.sync_all())
            .map_err(io)?;
        Ok(())
    }

    /// Classify the differences from `self` (running) to `new`.
    pub fn changes(&self, new: &Config) -> Changes {
        let mut c = Changes::default();
        if self.radio != new.radio {
            c.restart.push("radio");
        }
        if self.hpsdr != new.hpsdr {
            c.restart.push("hpsdr");
        }
        let (a, b) = (&self.web, &new.web);
        // what applies at once
        let fixed = |w: &Web| Web {
            default_freq: None,
            wf_budget: 0,
            metrics: false,
            trusted_proxies: Vec::new(),
            ..w.clone()
        };
        let web_restart = fixed(a) != fixed(b);
        if web_restart {
            c.restart.push("web");
        }
        c.live = a.default_freq != b.default_freq
            || a.wf_budget != b.wf_budget
            || a.metrics != b.metrics
            || a.trusted_proxies != b.trusted_proxies
            || self.limits != new.limits
            || self.auth != new.auth;
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_is_defaults_and_defaults_round_trip() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
        let c = Config::default();
        assert_eq!(Config::parse(&c.to_toml()).unwrap(), c);
    }

    #[test]
    fn partial_file_keeps_other_defaults() {
        let c = Config::parse(
            r#"
            [radio]
            backend = "iq"
            [radio.iq]
            source = "soapy:driver=hackrf"
            freq = 8.6e6
            loop = true
            gains = { LNA = 16, VGA = 20 }
            [web]
            tls = "off"
            "#,
        )
        .unwrap();
        assert_eq!(c.radio.backend, Backend::Iq);
        assert_eq!(c.radio.iq.freq, Some(8.6e6));
        assert!(c.radio.iq.loop_file);
        assert_eq!(c.radio.iq.gains["LNA"], 16.0);
        assert_eq!(c.radio.iq.rate, 7.68e6);
        assert_eq!(c.web.tls, TlsMode::Off);
        assert_eq!(c.web.listen, "0.0.0.0:8073");
        assert_eq!(c.hpsdr, Hpsdr::default());
    }

    #[test]
    fn typos_are_errors() {
        let e = Config::parse("[web]\nlisten_addr = \"x\"\n").unwrap_err();
        assert!(e.to_string().contains("listen_addr"), "{e}");
        assert!(Config::parse("[radio]\nbackend = \"usb\"\n").is_err());
    }

    #[test]
    fn save_is_atomic_and_load_reads_it_back() {
        let dir = std::env::temp_dir().join(format!("web888-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = dir.join("sub/web888d.toml");
        assert_eq!(
            Config::load(&p).unwrap(),
            Config::default(),
            "missing = defaults"
        );
        let mut c = Config::default();
        c.limits.per_ip = 2;
        c.save(&p).unwrap();
        assert_eq!(Config::load(&p).unwrap(), c);
        let names: Vec<_> = std::fs::read_dir(p.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["web888d.toml"], "no temp file left");
        std::fs::write(&p, "[[[").unwrap();
        assert!(matches!(Config::load(&p), Err(Error::Parse(..))));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_through_a_symlink_replaces_its_target() {
        let dir = std::env::temp_dir().join(format!("web888-config-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("etc")).unwrap();
        std::fs::create_dir_all(dir.join("config")).unwrap();
        let link = dir.join("etc/web888d.toml");
        std::os::unix::fs::symlink("../config/web888d.toml", &link).unwrap();
        let mut c = Config::default();
        c.limits.per_ip = 3;
        c.save(&link).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(Config::load(&dir.join("config/web888d.toml")).unwrap(), c);
        assert_eq!(Config::load(&link).unwrap(), c);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn networks_parse_and_match() {
        let n: IpNet = "10.0.0.0/8".parse().unwrap();
        assert!(n.contains("10.200.3.4".parse().unwrap()));
        assert!(!n.contains("11.0.0.1".parse().unwrap()));
        // a v4 peer seen on a v6 socket
        assert!(n.contains("::ffff:10.1.2.3".parse().unwrap()));
        let one: IpNet = "192.0.2.7".parse().unwrap();
        assert!(one.contains("192.0.2.7".parse().unwrap()));
        assert!(!one.contains("192.0.2.8".parse().unwrap()));
        let v6: IpNet = "fd00::/8".parse().unwrap();
        assert!(v6.contains("fd12::1".parse().unwrap()));
        assert!(!v6.contains("10.0.0.1".parse().unwrap()));
        let all: IpNet = "0.0.0.0/0".parse().unwrap();
        assert!(all.contains("203.0.113.9".parse().unwrap()));
        for bad in ["10.0.0.0/33", "nonsense", "10.0.0.0/x"] {
            assert!(bad.parse::<IpNet>().is_err(), "{bad}");
        }
        let c =
            Config::parse("[web]\ntrusted_proxies = [\"127.0.0.1\", \"10.0.0.0/8\"]\n").unwrap();
        assert_eq!(c.web.trusted_proxies.len(), 2);
        assert_eq!(Config::parse(&c.to_toml()).unwrap(), c);
        assert!(Config::parse("[web]\ntrusted_proxies = [\"x\"]\n").is_err());
    }

    #[test]
    fn changes_are_classified() {
        let a = Config::default();
        let mut b = a.clone();
        assert_eq!(a.changes(&b), Changes::default());
        b.limits.per_ip = 1;
        b.web.wf_budget = 10;
        b.web.metrics = true;
        b.web.trusted_proxies = vec!["10.0.0.1".parse().unwrap()];
        assert_eq!(
            a.changes(&b),
            Changes {
                live: true,
                restart: vec![]
            }
        );
        b.web.listen = "0.0.0.0:80".into();
        b.radio.backend = Backend::Sim;
        assert_eq!(a.changes(&b).restart, ["radio", "web"]);
    }
}
