// SPDX-License-Identifier: GPL-3.0-or-later
//! web888d.
//!
//! `serve`: run the radio (simulated, or the stock driver on a unit) and
//! offer its channels over HPSDR Protocol 1 (UDP 1024) and the web UI
//! (HTTPS/HTTP on 8073):
//!
//!   web888d serve --backend zynqsdr          # on the unit, stock server stopped
//!   web888d serve --backend sim --tone 7.0755e6:-40 --noise
//!   web888d serve --backend iq --iq file:cap.cs8 --iq-freq 10.5e6 --iq-loop
//!   web888d serve --backend iq --iq soapy:driver=hackrf --iq-freq 10.5e6 \
//!     --iq-gain LNA=16 --iq-gain VGA=20     (built with --features soapy)
//!
//! `passwd`: set the admin password (or `--listener`: the shared listener
//! password) in the configuration file; a running server picks it up:
//!
//!   web888d passwd
//!
//! `sim`: one demodulated channel to stdout (raw f32 LE), for listening:
//!
//!   web888d sim --tone 7.0755e6:-40 --noise --tune 7.074e6 --mode usb \
//!     | aplay -t raw -f FLOAT_LE -r 24000 -c 1

use clap::{Parser, Subcommand, ValueEnum};
use std::io::Write;
use std::net::UdpSocket;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use web888_config::{Backend, Config, TlsMode};
use web888_core::leases::{Channels, Class, Lease, Policy};
use web888_dsp::chan::{ChannelSpec, Droop, design};
use web888_dsp::demod::{Am, ssb};
use web888_dsp::ols::OverlapSave;
use web888_dsp::{C32, rx_to_c32};
use web888_radio::*;
use web888_sim::*;

#[derive(Parser)]
#[command(version, about = "web888-ng server for the Web-888 SDR")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run one receiver channel on the simulated radio.
    Sim(SimArgs),
    /// Run the radio and serve its channels (HPSDR Protocol 1, web UI).
    Serve(Box<ServeArgs>),
    /// Set the admin password (read from the terminal or stdin).
    Passwd(PasswdArgs),
}

#[derive(clap::Args)]
struct PasswdArgs {
    /// Configuration file to write the password hash to.
    #[arg(long, default_value = web888_config::DEFAULT_PATH)]
    config: PathBuf,
    /// Set the shared listener password instead (a private receiver).
    #[arg(long)]
    listener: bool,
    /// Remove the listener password (an open receiver again).
    #[arg(long, requires = "listener")]
    remove: bool,
}

/// `serve` options. Each overrides its key in the configuration file
/// (`--print-config` shows the result); flags can only switch things on
/// or off as named.
#[derive(clap::Args, Clone)]
struct ServeArgs {
    /// Configuration file (TOML; a missing file means all defaults).
    #[arg(long, default_value = web888_config::DEFAULT_PATH)]
    config: PathBuf,
    /// Print the effective configuration (file + options) and exit.
    #[arg(long)]
    print_config: bool,
    /// Radio: sim (simulated), zynqsdr (/dev/zynqsdr; stop the stock server
    /// first), iq (wideband IQ from an SDR or a recording, --iq ...).
    #[arg(long)]
    backend: Option<Backend>,
    /// Bitstream to load first (zynqsdr backend): a path, `auto` for the
    /// variant's file in /usr/share/web888 (web888-ng-os image) or else the
    /// stock one on the SD card, or `none` if it's already loaded.
    #[arg(long)]
    bitstream: Option<String>,
    /// EXPERIMENTAL (test unit only): reprogram the Si5351 ADC clock to this
    /// frequency before loading the bitstream. Default: keep the FSBL setting.
    #[arg(long)]
    si5351_adc: Option<f64>,
    /// RX channels from the channelizer's bins: how many (backend iq: its
    /// software bank; zynqsdr: the channelizer bitstream, 32 by default).
    #[arg(long)]
    bins: Option<usize>,
    /// HPSDR Protocol 1 listen address [0.0.0.0:1024].
    #[arg(long)]
    hpsdr: Option<String>,
    /// Board id reported to HPSDR clients (1 = Hermes, 5 = Orion, 6 = Hermes-Lite).
    #[arg(long)]
    hpsdr_board: Option<u8>,
    /// Receivers offered to HPSDR clients (max 7; each takes an RX channel
    /// lease while the client streams, limits.hpsdr_channels at most).
    #[arg(long)]
    hpsdr_rx: Option<usize>,
    /// Don't serve HPSDR.
    #[arg(long)]
    no_hpsdr: bool,
    /// Web UI listen address, HTTPS and plain HTTP on one port [0.0.0.0:8073].
    #[arg(long)]
    http: Option<String>,
    /// Don't serve the web UI.
    #[arg(long)]
    no_web: bool,
    /// Web UI files [/usr/share/web888d/ui].
    #[arg(long)]
    ui_dir: Option<PathBuf>,
    /// TLS certificate and key (PEM); default: a self-signed pair in --state-dir.
    #[arg(long, requires = "tls_key")]
    tls_cert: Option<PathBuf>,
    #[arg(long, requires = "tls_cert")]
    tls_key: Option<PathBuf>,
    /// Plain HTTP only (e.g. behind a TLS-terminating proxy).
    #[arg(long, conflicts_with = "tls_cert")]
    no_tls: bool,
    /// Where web888d keeps state, e.g. the self-signed certificate [/var/lib/web888d].
    #[arg(long)]
    state_dir: Option<PathBuf>,
    /// Initial web dial frequency in Hz (default: 7.074 MHz if in range).
    #[arg(long)]
    web_freq: Option<f64>,
    /// Most FFTs averaged per waterfall line, a CPU budget [30, 6 on armv7].
    #[arg(long)]
    wf_budget: Option<usize>,
    /// Simulated scene (sim backend): tones FREQ_HZ:DBFS (replaces the file's).
    #[arg(long = "tone", value_parser = parse_tone)]
    tones: Vec<(f64, f64)>,
    #[arg(long)]
    noise: bool,
    /// IQ backend input: `file:PATH` (.cs8 as from hackrf_transfer, .cs16,
    /// .cf32) or `soapy:ARGS` (e.g. soapy:driver=hackrf).
    #[arg(long)]
    iq: Option<String>,
    /// IQ input centre frequency (Hz); the clean band is +-0.45 x rate,
    /// tuning works to +-1.5 x rate (out-of-band signals folded in).
    #[arg(long)]
    iq_freq: Option<f64>,
    /// IQ input sample rate: 122.88 MHz / 8, 16, ..., 256 [7.68e6].
    #[arg(long)]
    iq_rate: Option<f64>,
    /// Device gain element NAME=DB (soapy; repeatable, replaces the file's),
    /// e.g. LNA=16, VGA=20, AMP=0.
    #[arg(long = "iq-gain", value_parser = parse_gain)]
    iq_gains: Vec<(String, f64)>,
    /// Device analog bandwidth in Hz (soapy; default: the driver's choice).
    #[arg(long)]
    iq_bw: Option<f64>,
    /// Device antenna port (soapy).
    #[arg(long)]
    iq_antenna: Option<String>,
    /// Start a file again at its end.
    #[arg(long)]
    iq_loop: bool,
    #[arg(short, long)]
    verbose: bool,
}

impl ServeArgs {
    /// Apply the options given on the command line over `c`.
    fn apply(&self, c: &mut Config) {
        fn set<T: Clone>(dst: &mut T, v: &Option<T>) {
            if let Some(v) = v {
                *dst = v.clone();
            }
        }
        let r = &mut c.radio;
        set(&mut r.backend, &self.backend);
        set(&mut r.bitstream, &self.bitstream);
        if self.si5351_adc.is_some() {
            r.si5351_adc = self.si5351_adc;
        }
        if self.bins.is_some() {
            r.bins = self.bins;
        }
        if !self.tones.is_empty() {
            r.sim.tones = self.tones.iter().map(|&(f, l)| [f, l]).collect();
        }
        r.sim.noise |= self.noise;
        if self.iq.is_some() {
            r.iq.source = self.iq.clone();
        }
        if self.iq_freq.is_some() {
            r.iq.freq = self.iq_freq;
        }
        set(&mut r.iq.rate, &self.iq_rate);
        if !self.iq_gains.is_empty() {
            r.iq.gains = self.iq_gains.iter().cloned().collect();
        }
        set(&mut r.iq.bandwidth, &self.iq_bw);
        if self.iq_antenna.is_some() {
            r.iq.antenna = self.iq_antenna.clone();
        }
        r.iq.loop_file |= self.iq_loop;

        let h = &mut c.hpsdr;
        set(&mut h.listen, &self.hpsdr);
        set(&mut h.board, &self.hpsdr_board);
        set(&mut h.receivers, &self.hpsdr_rx);
        if self.no_hpsdr {
            h.enabled = false;
        }

        let w = &mut c.web;
        set(&mut w.listen, &self.http);
        set(&mut w.ui_dir, &self.ui_dir);
        set(&mut w.state_dir, &self.state_dir);
        if self.tls_cert.is_some() {
            w.tls = TlsMode::Files;
            w.tls_cert = self.tls_cert.clone();
            w.tls_key = self.tls_key.clone();
        }
        if self.no_tls {
            w.tls = TlsMode::Off;
        }
        if self.web_freq.is_some() {
            w.default_freq = self.web_freq;
        }
        set(&mut w.wf_budget, &self.wf_budget);
        if self.no_web {
            w.enabled = false;
        }
    }

    /// The configuration file with the command line applied.
    fn effective(&self) -> AnyResult<Config> {
        let mut c = Config::load(&self.config)?;
        self.apply(&mut c);
        Ok(c)
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum Mode {
    Usb,
    Lsb,
    Am,
    Iq,
}

#[derive(clap::Args)]
struct SimArgs {
    /// Tone at the antenna, FREQ_HZ:DBFS (repeatable).
    #[arg(long = "tone", value_parser = parse_tone)]
    tones: Vec<(f64, f64)>,
    /// AM station FREQ_HZ:DBFS (1 kHz tone, 50 % depth; repeatable).
    #[arg(long = "am", value_parser = parse_tone)]
    am: Vec<(f64, f64)>,
    /// Add the typical ADC noise floor.
    #[arg(long)]
    noise: bool,
    /// Hardware NCO frequency (Hz).
    #[arg(long)]
    tune: f64,
    #[arg(long, value_enum, default_value = "usb")]
    mode: Mode,
    /// Hardware output rate (12000/24000/48000).
    #[arg(long, default_value_t = 24_000.0)]
    rate: f64,
    /// Stop after this many seconds (0 = run until killed).
    #[arg(long, default_value_t = 0.0)]
    seconds: f64,
    /// Simulated ADC clock error in ppm.
    #[arg(long, default_value_t = 0.0)]
    ppm: f64,
}

fn parse_tone(s: &str) -> std::result::Result<(f64, f64), String> {
    let (f, l) = s.split_once(':').ok_or("expected FREQ_HZ:DBFS")?;
    Ok((
        f.parse().map_err(|e| format!("{e}"))?,
        l.parse().map_err(|e| format!("{e}"))?,
    ))
}

fn parse_gain(s: &str) -> std::result::Result<(String, f64), String> {
    let (n, g) = s.split_once('=').ok_or("expected NAME=DB")?;
    Ok((n.to_string(), g.parse().map_err(|e| format!("{e}"))?))
}

/// The radio, and in bins mode the tap for waterfalls from the bins.
type Opened = (Box<dyn Radio>, Option<web888_dsp::binrx::BinTap>);

/// `radio`'s bins as `n` RX channels.
fn bin_rx<R: Radio + web888_radio::bins::BinRadio + 'static>(
    radio: R,
    n: usize,
) -> AnyResult<Opened> {
    let mut r = web888_dsp::binrx::BinRx::new(radio, n)?;
    let tap = r.tap();
    Ok((Box::new(r), Some(tap)))
}

fn open_iq(a: &web888_config::Iq, bins: Option<usize>) -> AnyResult<Opened> {
    use web888_iq::source::{FileSource, Format, IqSource};
    use web888_iq::{IqConfig, IqRadio};
    let spec = a
        .source
        .as_deref()
        .ok_or("backend iq needs --iq file:PATH or --iq soapy:ARGS ([radio.iq] source)")?;
    let fc = a
        .freq
        .ok_or("backend iq needs --iq-freq, the input centre frequency ([radio.iq] freq)")?;
    let mut cfg = IqConfig::new(fc, a.rate);
    cfg.bins = bins.is_some();
    let src: Box<dyn IqSource> = if let Some(p) = spec.strip_prefix("file:") {
        let path = std::path::Path::new(p);
        let fmt = Format::from_path(path).ok_or("IQ file: use extension .cs8, .cs16 or .cf32")?;
        Box::new(FileSource::open(path, fmt, a.loop_file).map_err(|e| format!("{p}: {e}"))?)
    } else if let Some(args) = spec.strip_prefix("soapy:") {
        open_soapy(args, fc, a)?
    } else {
        return Err("--iq: expected file:PATH or soapy:ARGS".into());
    };
    eprintln!(
        "iq: {spec}, {:.3} MS/s around {:.6} MHz (clean band {:.3}..{:.3} MHz)",
        a.rate / 1e6,
        fc / 1e6,
        (fc - 0.45 * a.rate) / 1e6,
        (fc + 0.45 * a.rate) / 1e6
    );
    let radio = IqRadio::new(cfg, src)?;
    match bins {
        Some(n) => {
            eprintln!("bins: {n} RX channels from the software bank's 30 kHz bins");
            bin_rx(radio, n)
        }
        None => Ok((Box::new(radio), None)),
    }
}

#[cfg(feature = "soapy")]
fn open_soapy(
    args: &str,
    fc: f64,
    a: &web888_config::Iq,
) -> AnyResult<Box<dyn web888_iq::source::IqSource>> {
    use web888_iq::source::{SoapyConfig, SoapySource};
    let c = SoapyConfig {
        args: args.to_string(),
        center_hz: fc,
        rate_hz: a.rate,
        bandwidth_hz: a.bandwidth,
        gains: a.gains.iter().map(|(k, v)| (k.clone(), *v)).collect(),
        antenna: a.antenna.clone(),
    };
    Ok(Box::new(
        SoapySource::open(&c).map_err(|e| format!("soapy:{args}: {e}"))?,
    ))
}

#[cfg(not(feature = "soapy"))]
fn open_soapy(
    _: &str,
    _: f64,
    _: &web888_config::Iq,
) -> AnyResult<Box<dyn web888_iq::source::IqSource>> {
    Err("this web888d was built without SoapySDR (cargo build --features soapy)".into())
}

fn main() {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Sim(a) => {
            if let Err(e) = run_sim(a) {
                eprintln!("web888d: {e}");
                std::process::exit(1);
            }
        }
        Cmd::Serve(a) => {
            if let Err(e) = run_serve(*a) {
                eprintln!("web888d: {e}");
                std::process::exit(1);
            }
        }
        Cmd::Passwd(a) => {
            if let Err(e) = run_passwd(a) {
                eprintln!("web888d: {e}");
                std::process::exit(1);
            }
        }
    }
}

type AnyResult<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Read a line from stdin; without echo, after `prompt`, on a terminal.
fn read_password(prompt: &str) -> AnyResult<String> {
    // SAFETY: isatty/tcgetattr/tcsetattr on fd 0 with a zeroed termios
    let tty = unsafe { libc::isatty(0) } == 1;
    let mut old: libc::termios = unsafe { std::mem::zeroed() };
    if tty {
        eprint!("{prompt}");
        unsafe {
            libc::tcgetattr(0, &mut old);
            let mut t = old;
            t.c_lflag &= !libc::ECHO;
            libc::tcsetattr(0, libc::TCSANOW, &t);
        }
    }
    let mut line = String::new();
    let r = std::io::stdin().read_line(&mut line);
    if tty {
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &old) };
        eprintln!();
    }
    r?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

fn run_passwd(a: PasswdArgs) -> AnyResult<()> {
    use web888_web::auth;
    let mut c = Config::load(&a.config)?;
    let what = if a.listener { "listener" } else { "admin" };
    if a.remove {
        c.auth.listener_hash = None;
    } else {
        let pw = read_password(&format!("New {what} password: "))?;
        // SAFETY: isatty has no preconditions
        if unsafe { libc::isatty(0) } == 1 && read_password("Again: ")? != pw {
            return Err("the passwords differ".into());
        }
        let min = if a.listener { 1 } else { auth::ADMIN_MIN_LEN };
        if pw.chars().count() < min {
            return Err(format!("the {what} password needs at least {min} characters").into());
        }
        let h = auth::hash(&pw);
        if a.listener {
            c.auth.listener_hash = Some(h);
        } else {
            c.auth.admin_hash = Some(h);
        }
    }
    c.save(&a.config)?;
    eprintln!(
        "{what} password {} in {}",
        if a.remove { "removed" } else { "set" },
        a.config.display()
    );
    Ok(())
}

/// A first admin password put on the SD card: applied at start, then the
/// file is deleted (also a way back in after a forgotten password). Where
/// it can't be deleted (the image mounts /boot read-only), a fingerprint
/// in the state directory remembers it was applied, so it isn't applied
/// again over a password changed since; a file with new content is.
const FIRST_PASSWORD: &str = "/boot/web888d-admin";
const FIRST_PASSWORD_SEEN: &str = "boot-admin-applied";

fn first_password(
    p: &std::path::Path,
    config: &std::path::Path,
    state_dir: &std::path::Path,
) -> AnyResult<()> {
    let Ok(s) = std::fs::read_to_string(p) else {
        return Ok(());
    };
    let seen = state_dir.join(FIRST_PASSWORD_SEEN);
    if std::fs::read_to_string(&seen).is_ok_and(|h| web888_web::auth::verify(h.trim(), &s)) {
        return Ok(());
    }
    let pw = s.lines().next().unwrap_or("").trim_end_matches('\r');
    if pw.chars().count() < web888_web::auth::ADMIN_MIN_LEN {
        eprintln!(
            "auth: {}: too short (at least {} characters), ignored",
            p.display(),
            web888_web::auth::ADMIN_MIN_LEN
        );
    } else {
        let mut c = Config::load(config)?;
        c.auth.admin_hash = Some(web888_web::auth::hash(pw));
        c.save(config)?;
        eprintln!("auth: admin password set from {}", p.display());
    }
    if let Err(e) = std::fs::remove_file(p) {
        std::fs::create_dir_all(state_dir)?;
        std::fs::write(&seen, web888_web::auth::hash(&s))?;
        eprintln!(
            "auth: can't delete {} ({e}); it won't be applied again unless it changes",
            p.display()
        );
    }
    Ok(())
}

/// The station as reported: the configured one, with gpsd's locator
/// once there is one.
fn station(
    s: &web888_config::Station,
    gps: &std::sync::Mutex<web888_skim::gpsd::Status>,
) -> web888_config::Station {
    let mut s = s.clone();
    if !s.gpsd.is_empty()
        && let Some(l) = gps.lock().unwrap().locator.clone()
    {
        s.locator = l;
    }
    s
}

/// Background decoders: bands from the config (and its reloads), run in
/// their UTC hours; spots are logged.
fn start_skimmers(
    cfg: &Config,
    live: &tokio::sync::watch::Receiver<Arc<Config>>,
    channels: Arc<Channels>,
    pool: Arc<web888_core::pool::DspPool>,
    rx_rate: f64,
    radio_live: bool,
) -> AnyResult<web888_web::session::Skim> {
    let threads = pool.workers();
    let sk = web888_skim::Skimmers::new(channels, pool, rx_rate, radio_live, threads)?;
    let hour = || {
        (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            / 3600
            % 24) as u32
    };
    // before the bands start, so syslog and MQTT have their start events
    {
        let live = live.clone();
        web888_skim::syslog::spawn(sk.bus.clone(), move || live.borrow().syslog.clone())?;
    }
    {
        let live = live.clone();
        web888_skim::mqtt::spawn(sk.bus.clone(), move || live.borrow().mqtt.clone())?;
    }
    sk.apply(&cfg.skimmer, hour());
    if !cfg.skimmer.is_empty() && !radio_live {
        eprintln!(
            "skim: this radio isn't live (a recording or simulation): spots won't be uploaded"
        );
    }
    let mut cfg_rx = live.clone();
    let s = sk.clone();
    std::thread::Builder::new()
        .name("skim-cfg".into())
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(10));
                let c = cfg_rx.borrow_and_update().clone();
                s.apply(&c.skimmer, hour());
            }
        })?;
    let gps = {
        let live = live.clone();
        web888_skim::gpsd::spawn(move || live.borrow().station.gpsd.clone())?
    };
    let psk = {
        let (live, gps) = (live.clone(), gps.clone());
        web888_skim::pskreporter::spawn(
            sk.bus.clone(),
            move || {
                let c = live.borrow().clone();
                (station(&c.station, &gps), c.report.clone())
            },
            env!("CARGO_PKG_VERSION"),
        )?
    };
    let wsprnet = {
        let (live, gps) = (live.clone(), gps.clone());
        web888_skim::wsprnet::spawn(
            sk.bus.clone(),
            move || {
                let c = live.borrow().clone();
                (station(&c.station, &gps), c.report.clone())
            },
            env!("CARGO_PKG_VERSION"),
        )?
    };
    let mut spots = sk.bus.subscribe();
    std::thread::Builder::new()
        .name("skim-log".into())
        .spawn(move || {
            loop {
                match spots.blocking_recv() {
                    Ok(s) => eprintln!("spot: {}", s.log_line()),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => return,
                }
            }
        })?;
    Ok(web888_web::session::Skim {
        bus: sk.bus.clone(),
        skimmers: Some(sk),
        pskreporter: Some(psk),
        wsprnet: Some(wsprnet),
        gpsd: Some(gps),
        live: radio_live,
    })
}

/// The widest waterfall span the UI may ask for (as a decimation): a view
/// is captured at half its decimation (`web888_web::wfsvc`), and no span
/// is wider than the tunable range.
fn wf_span_decim_min(caps: &Caps) -> u16 {
    let width = caps.rf_range.end() - caps.rf_range.start();
    let mut d = (2 * caps.wf_decim_min).max(2);
    while d < caps.wf_decim_max && caps.adc_hz / (d as f64) > width {
        d *= 2;
    }
    d
}

fn open_radio(a: &web888_config::Radio) -> AnyResult<Opened> {
    match a.backend {
        Backend::Sim => {
            let mut scene = Scene::new();
            for &[f, l] in &a.sim.tones {
                scene = scene.tone(f, l);
            }
            if a.sim.noise {
                scene = scene.adc_noise();
            }
            Ok((
                Box::new(SimRadio::new(SimConfig::hf(scene, Pacing::RealTime))),
                None,
            ))
        }
        Backend::Iq => open_iq(&a.iq, a.bins),
        Backend::Zynqsdr => {
            use web888_zynqsdr::{ZynqConfig, ZynqsdrRadio, bitstream, dev::ZynqsdrDev, si5351};
            let mut cfg = ZynqConfig::default();
            if let Some(f) = a.si5351_adc {
                let p =
                    si5351::plan(f, si5351::REF_HZ).ok_or("no Si5351 plan for that frequency")?;
                si5351::program("/dev/i2c-0", &p).map_err(|e| format!("si5351: {e}"))?;
                eprintln!(
                    "si5351: CLK0 {:.3} Hz (PLLA {}+{}/{}, MS0 {})",
                    p.out_hz, p.pll.a, p.pll.b, p.pll.c, p.ms.a
                );
                cfg.adc_hz = p.out_hz;
                if p.out_hz < 100e6 {
                    cfg.variant = Variant::Vhf; // stock rule: ADC < 100 MHz uses the VHF bitstream
                }
            }
            let bit = match a.bitstream.as_str() {
                "none" => None,
                "auto" => Some(bitstream::auto_path(
                    cfg.variant,
                    &[std::path::Path::new(bitstream::IMAGE_DIR)],
                )),
                p => Some(std::path::PathBuf::from(p)),
            };
            if let Some(bit) = bit {
                use std::path::Path;
                let (n, via) = if bitstream::has_devcfg() {
                    (
                        bitstream::load(&bit, Path::new(bitstream::XDEVCFG)),
                        "devcfg",
                    )
                } else {
                    let n = bitstream::load_fpga_mgr(
                        &bit,
                        Path::new(bitstream::FPGA_MGR),
                        Path::new(bitstream::FW_DIR),
                        bitstream::FW_NAME,
                    );
                    (n, "fpga_manager")
                };
                let n = n.map_err(|e| format!("loading {} ({via}): {e}", bit.display()))?;
                eprintln!("bitstream: {} ({n} bytes, {via})", bit.display());
            }
            let dev = ZynqsdrDev::open(ZynqsdrDev::PATH)
                .map_err(|e| format!("{}: {e} (is the stock server running?)", ZynqsdrDev::PATH))?;
            let radio = ZynqsdrRadio::new(dev, cfg)?;
            let sig = radio.identity().signature;
            if web888_zynqsdr::is_channelizer(sig) {
                let n = a.bins.unwrap_or(32);
                eprintln!("bins: {n} RX channels from the channelizer bitstream");
                bin_rx(radio, n)
            } else if a.bins.is_some() {
                Err(format!("bins: needs the channelizer bitstream (signature 0x{sig:08x})").into())
            } else {
                Ok((Box::new(radio), None))
            }
        }
    }
}

fn eth0_mac() -> [u8; 6] {
    let mut mac = [0x02, 0, 0, 0x88, 0x88, 0x01];
    if let Ok(s) = std::fs::read_to_string("/sys/class/net/eth0/address") {
        for (k, b) in s.trim().split(':').take(6).enumerate() {
            mac[k] = u8::from_str_radix(b, 16).unwrap_or(0);
        }
    }
    mac
}

/// Set by SIGHUP: reload the configuration now.
static RELOAD: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sighup(_: libc::c_int) {
    RELOAD.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Applies the configuration file to the running server: settings that
/// apply at once go out through `tx`, the others are logged as needing a
/// restart.
struct Reloader {
    args: ServeArgs,
    running: std::sync::Mutex<Config>,
    tx: tokio::sync::watch::Sender<Arc<Config>>,
    channels: Arc<Channels>,
}

impl Reloader {
    fn reload(&self) -> std::result::Result<(), String> {
        let new = self.args.effective().map_err(|e| e.to_string())?;
        let mut running = self.running.lock().unwrap();
        let ch = running.changes(&new);
        if !ch.restart.is_empty() {
            eprintln!(
                "config: changes to [{}] take effect after a restart",
                ch.restart.join("], [")
            );
        }
        if ch.live {
            eprintln!("config: reloaded");
            self.channels.set_policy(Policy::from_limits(&new.limits));
            let _ = self.tx.send(Arc::new(new.clone()));
        }
        *running = new;
        Ok(())
    }
}

/// Reload the configuration when the file changes (checked every 2 s) or
/// on SIGHUP. A broken file is logged and ignored.
fn watch_config(r: Arc<Reloader>) {
    // SAFETY: the handler only stores to an atomic
    unsafe { libc::signal(libc::SIGHUP, on_sighup as *const () as libc::sighandler_t) };
    let mtime = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let mut seen = mtime(&r.args.config);
    loop {
        std::thread::sleep(Duration::from_secs(2));
        let now = mtime(&r.args.config);
        let hup = RELOAD.swap(false, std::sync::atomic::Ordering::Relaxed);
        if now == seen && !hup {
            continue;
        }
        seen = now;
        if let Err(e) = r.reload() {
            eprintln!("config: not reloaded: {e}");
        }
    }
}

/// The HPSDR server's host: one RX channel lease per receiver while the
/// client streams; receivers without a channel get zeros.
struct HpsdrHost {
    channels: Arc<Channels>,
    hw: mpsc::Sender<web888_hw::HwCmd>,
    /// Each receiver's frequency, kept while it has no lease.
    hz: Vec<f64>,
    leases: Vec<Lease>,
}

impl web888_hpsdr::server::Host for HpsdrHost {
    fn control(&mut self, c: web888_hpsdr::server::Control) {
        use web888_hpsdr::server::Control;
        match c {
            Control::Tune { rx, hz } => {
                if let Some(f) = self.hz.get_mut(rx) {
                    *f = hz;
                }
                if let Some(l) = self.leases.get(rx) {
                    l.tune(hz);
                }
            }
            Control::Atten { db } => {
                let _ = self.hw.send(web888_hw::HwCmd::Atten { db });
            }
            Control::Receivers(n) => {
                let n = n.min(self.hz.len());
                self.leases.truncate(n);
                while self.leases.len() < n {
                    let rx = self.leases.len();
                    let l = self.channels.acquire(
                        Class::Protocol,
                        &format!("hpsdr rx{rx}"),
                        self.hz[rx],
                    );
                    if l.channel().is_none() {
                        eprintln!("hpsdr: rx{rx} waits for a channel (zeros until then)");
                    }
                    self.leases.push(l);
                }
            }
        }
    }

    fn channel(&mut self, rx: usize) -> Option<usize> {
        self.leases.get(rx)?.channel()
    }
}

fn run_serve(a: ServeArgs) -> AnyResult<()> {
    use web888_hpsdr::{proto::Board, server};
    if !a.print_config {
        first_password(
            std::path::Path::new(FIRST_PASSWORD),
            &a.config,
            &a.effective()?.web.state_dir,
        )?;
    }
    let cfg = a.effective()?;
    if a.print_config {
        print!("{}", cfg.to_toml());
        return Ok(());
    }
    if std::fs::metadata(&a.config).is_ok() {
        eprintln!("config: {}", a.config.display());
    } else {
        eprintln!("config: {} not found, using defaults", a.config.display());
    }
    let (mut radio, bin_tap) = open_radio(&cfg.radio)?;
    let caps = radio.caps();
    let id = radio.identity();
    eprintln!(
        "radio: {} signature {:#010x}, {} RX / {} WF channels, ADC {:.0} Hz",
        id.backend, id.signature, caps.rx_channels, caps.wf_channels, caps.adc_hz
    );
    // HPSDR P1 is served at 48 kS/s; the web audio chain decimates from it
    let r2 = r2_for_rate(caps.adc_hz, 48_000.0);
    radio.set_rx_rate(r2)?;

    let stop = Arc::new(AtomicBool::new(false));
    let status = Arc::new(web888_hw::HwStatus::default());
    let (ctl_tx, ctl_rx) = mpsc::channel();
    let channels = Channels::new(
        caps.rx_channels,
        Policy::from_limits(&cfg.limits),
        ctl_tx.clone(),
    );
    let (live_tx, live_rx) = tokio::sync::watch::channel(Arc::new(cfg.clone()));
    let reloader = Arc::new(Reloader {
        args: a.clone(),
        running: std::sync::Mutex::new(cfg.clone()),
        tx: live_tx,
        channels: channels.clone(),
    });
    {
        let r = reloader.clone();
        std::thread::Builder::new()
            .name("config".into())
            .spawn(move || watch_config(r))?;
    }

    let mut threads = Vec::new();
    let pool = Arc::new(web888_core::pool::DspPool::per_cpu(&ctl_tx)?);
    eprintln!("dsp: {} pool workers", pool.workers());
    let skim = start_skimmers(
        &cfg,
        &live_rx,
        channels.clone(),
        pool.clone(),
        rx_rate(caps.adc_hz, r2),
        id.live,
    )?;
    if cfg.hpsdr.enabled {
        let h = &cfg.hpsdr;
        let (sub, blk_rx, _drops) = web888_hw::Subscriber::new(64);
        ctl_tx.send(web888_hw::HwCmd::SubscribeRx(sub))?;
        let sock = UdpSocket::bind(&h.listen).map_err(|e| format!("bind {}: {e}", h.listen))?;
        let max_rx = h.receivers.min(caps.rx_channels).min(7);
        let scfg = server::Config {
            board: Board {
                mac: eth0_mac(),
                code_version: 1,
                board_id: h.board,
            },
            max_rx,
        };
        let mut srv = server::Server::new(sock, scfg)?;
        eprintln!(
            "hpsdr: listening on {} ({max_rx} receivers)",
            srv.local_addr()?
        );
        let srv_stop = stop.clone();
        let host = HpsdrHost {
            channels: channels.clone(),
            hw: ctl_tx.clone(),
            hz: vec![7_074_000.0; max_rx],
            leases: Vec::new(),
        };
        threads.push(
            std::thread::Builder::new()
                .name("hpsdr".into())
                .spawn(move || srv.run(blk_rx, host, srv_stop))?,
        );
    }

    if cfg.web.enabled {
        let w = &cfg.web;
        let rf = caps.rf_range.clone();
        let shared = Arc::new(web888_web::session::Shared {
            status: status.clone(),
            radio: web888_web::proto::RadioInfo {
                backend: id.backend.into(),
                variant: format!("{:?}", caps.variant).to_lowercase(),
                adc_hz: caps.adc_hz,
                rx_rate: rx_rate(caps.adc_hz, r2),
                rf_lo: *rf.start(),
                rf_hi: *rf.end(),
                wf_decim_min: wf_span_decim_min(&caps),
                wf_decim_max: caps.wf_decim_max,
            },
            channels: channels.clone(),
            waterfalls: match bin_tap {
                Some(tap) => web888_web::wfsvc::Waterfalls::from_bins(
                    tap,
                    caps.adc_hz,
                    ctl_tx.clone(),
                    pool.clone(),
                ),
                None => web888_web::wfsvc::Waterfalls::new(
                    caps.wf_channels,
                    caps.adc_hz,
                    caps.wf_block,
                    ctl_tx.clone(),
                    pool.clone(),
                ),
            },
            pool,
            rx_r2: r2,
            live: live_rx,
            ips: Default::default(),
            sessions: Default::default(),
            next_session: Default::default(),
            users: Default::default(),
            admin: web888_web::admin::Admin::new(web888_web::admin::Hooks {
                config_path: Some(a.config.clone()),
                reload: Some(Box::new(move || reloader.reload())),
                restart: Some(Box::new(|| {
                    eprintln!("web888d: restart requested by the admin");
                    // non-zero: systemd's Restart=on-failure starts it again
                    std::process::exit(75);
                })),
                overlay: Some(Box::new({
                    let a = a.clone();
                    move |c| a.apply(c)
                })),
            }),
            skim: Some(skim),
            limit_unit: Duration::from_secs(60),
        });
        let tls = match w.tls {
            TlsMode::Off => web888_web::Tls::Off,
            TlsMode::Files => web888_web::Tls::Files {
                cert: w.tls_cert.clone().ok_or("tls = \"files\" needs tls_cert")?,
                key: w.tls_key.clone().ok_or("tls = \"files\" needs tls_key")?,
            },
            TlsMode::SelfSigned => web888_web::Tls::SelfSigned(w.state_dir.clone()),
        };
        let acceptor = web888_web::tls::acceptor(&tls).map_err(|e| format!("tls: {e}"))?;
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("web")
            .enable_all()
            .build()?;
        let listener = rt
            .block_on(tokio::net::TcpListener::bind(&w.listen))
            .map_err(|e| format!("bind {}: {e}", w.listen))?;
        eprintln!(
            "web: {}://{} (UI from {})",
            if acceptor.is_some() { "https" } else { "http" },
            listener.local_addr()?,
            w.ui_dir.display()
        );
        let app = web888_web::router(shared, w.ui_dir.clone());
        let web_stop = stop.clone();
        threads.push(
            std::thread::Builder::new()
                .name("web".into())
                .spawn(move || {
                    if let Err(e) = rt.block_on(web888_web::serve(listener, app, acceptor)) {
                        eprintln!("web: {e}");
                        web_stop.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                })?,
        );
    }
    drop(ctl_tx);

    let hw = web888_hw::HwLoop {
        radio,
        ctl: ctl_rx,
        stop: stop.clone(),
        status,
        verbose: a.verbose,
    };
    let res = hw.run();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    // the web thread runs until the process exits
    drop(threads);
    res.map_err(|e| e.into())
}

fn run_sim(a: SimArgs) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut scene = Scene::new();
    for (f, l) in a.tones {
        scene = scene.tone(f, l);
    }
    for (f, l) in a.am {
        scene = scene.am(f, l, 1000.0, 0.5);
    }
    if a.noise {
        scene = scene.adc_noise();
    }
    let mut cfg = SimConfig::hf(scene, Pacing::RealTime);
    cfg.adc_ppm = a.ppm;
    let mut radio = SimRadio::new(cfg);

    let r2 = r2_for_rate(ADC_HZ_HF, a.rate);
    radio.set_rx_rate(r2)?;
    radio.tune_rx(0, a.tune)?;
    let fs = rx_rate(ADC_HZ_HF, r2);
    let droop = Droop::Rx {
        adc_hz: ADC_HZ_HF,
        r2,
    };
    let spec = match a.mode {
        Mode::Usb | Mode::Iq => ChannelSpec::usb(300.0, 2700.0),
        Mode::Lsb => ChannelSpec::lsb(300.0, 2700.0),
        Mode::Am => ChannelSpec::am(5000.0),
    };
    let taps = design(spec, fs, droop);
    let mut ols = OverlapSave::new(&taps);
    let mut am = Am::new(fs);
    eprintln!(
        "web888d sim: {} channels, {:.0} S/s (r2 {}), NCO {:.3} Hz, {} taps",
        radio.caps().rx_channels,
        fs,
        r2,
        a.tune,
        taps.len()
    );

    let n = radio.caps().rx_channels;
    let mut buf = vec![Iq32::default(); 4096 * n];
    let mut ch = Vec::<C32>::new();
    let mut filt = Vec::<C32>::new();
    let mut audio = Vec::<f32>::new();
    let mut bytes = Vec::<u8>::new();
    let mut out = std::io::stdout().lock();
    let t0 = Instant::now();
    let mut last = Instant::now();
    let mut peak = 0f32;
    loop {
        let got = radio.read_rx(&mut buf)?;
        if got == 0 {
            std::thread::sleep(Duration::from_millis(10));
        } else {
            ch.clear();
            ch.extend((0..got).map(|f| rx_to_c32(buf[f * n])));
            filt.clear();
            ols.process(&ch, &mut filt);
            bytes.clear();
            match a.mode {
                Mode::Iq => {
                    for z in &filt {
                        peak = peak.max(z.norm());
                        bytes.extend_from_slice(&z.re.to_le_bytes());
                        bytes.extend_from_slice(&z.im.to_le_bytes());
                    }
                }
                _ => {
                    audio.clear();
                    match a.mode {
                        Mode::Am => am.process(&filt, &mut audio),
                        _ => ssb(&filt, &mut audio),
                    }
                    for s in &audio {
                        peak = peak.max(s.abs());
                        bytes.extend_from_slice(&s.to_le_bytes());
                    }
                }
            }
            if out.write_all(&bytes).is_err() {
                return Ok(()); // reader went away
            }
        }
        if let Some(p) = radio.pps()? {
            eprintln!("pps: {p} ADC clocks");
        }
        if last.elapsed() >= Duration::from_secs(1) {
            last = Instant::now();
            eprintln!(
                "peak {:6.1} dBFS, overruns {}",
                20.0 * (peak.max(1e-12) as f64).log10(),
                radio.stats().rx_overrun_frames
            );
            peak = 0.0;
        }
        if a.seconds > 0.0 && t0.elapsed().as_secs_f64() >= a.seconds {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use web888_web::auth::verify;

    #[test]
    fn a_boot_password_that_cant_be_deleted_applies_once_per_content() {
        let dir = std::env::temp_dir().join(format!("web888d-bootpw-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let boot = dir.join("boot");
        std::fs::create_dir_all(&boot).unwrap();
        let file = boot.join("web888d-admin");
        let (config, state) = (dir.join("c.toml"), dir.join("state"));
        let admin = || Config::load(&config).unwrap().auth.admin_hash.unwrap();
        let ro = |on: bool| {
            let mode = if on { 0o555 } else { 0o755 };
            std::fs::set_permissions(&boot, std::fs::Permissions::from_mode(mode)).unwrap();
        };

        std::fs::write(&file, "first-password\n").unwrap();
        ro(true); // as the image's read-only /boot
        first_password(&file, &config, &state).unwrap();
        assert!(verify(&admin(), "first-password"));
        assert!(file.exists());

        // changed since (e.g. in the admin page): the old file doesn't undo it
        let mut c = Config::load(&config).unwrap();
        c.auth.admin_hash = Some(web888_web::auth::hash("changed-later"));
        c.save(&config).unwrap();
        first_password(&file, &config, &state).unwrap();
        assert!(verify(&admin(), "changed-later"));

        // a new file is a reset
        ro(false);
        std::fs::write(&file, "reset-password\n").unwrap();
        ro(true);
        first_password(&file, &config, &state).unwrap();
        assert!(verify(&admin(), "reset-password"));

        // where it can be deleted, it is
        ro(false);
        first_password(&file, &config, &state).unwrap();
        assert!(verify(&admin(), "reset-password"));
        std::fs::write(&file, "deleted-password\n").unwrap();
        first_password(&file, &config, &state).unwrap();
        assert!(!file.exists());
        assert!(verify(&admin(), "deleted-password"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
