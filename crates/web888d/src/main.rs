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
    /// HPSDR Protocol 1 listen address [0.0.0.0:1024].
    #[arg(long)]
    hpsdr: Option<String>,
    /// Board id reported to HPSDR clients (1 = Hermes, 5 = Orion, 6 = Hermes-Lite).
    #[arg(long)]
    hpsdr_board: Option<u8>,
    /// Receivers offered to HPSDR clients (max 7; the web user's channel
    /// is not offered).
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
    /// RX channel for the web user (default: the last one).
    #[arg(long)]
    web_rx: Option<usize>,
    /// Initial web dial frequency in Hz (default: 7.074 MHz if in range).
    #[arg(long)]
    web_freq: Option<f64>,
    /// Most FFTs averaged per waterfall line, a CPU budget [30].
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
        if self.web_rx.is_some() {
            w.rx_channel = self.web_rx;
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

fn open_iq(a: &web888_config::Iq) -> AnyResult<Box<dyn Radio>> {
    use web888_iq::source::{FileSource, Format, IqSource};
    use web888_iq::{IqConfig, IqRadio};
    let spec = a
        .source
        .as_deref()
        .ok_or("backend iq needs --iq file:PATH or --iq soapy:ARGS ([radio.iq] source)")?;
    let fc = a
        .freq
        .ok_or("backend iq needs --iq-freq, the input centre frequency ([radio.iq] freq)")?;
    let cfg = IqConfig::new(fc, a.rate);
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
    Ok(Box::new(IqRadio::new(cfg, src)?))
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
    }
}

type AnyResult<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn open_radio(a: &web888_config::Radio) -> AnyResult<Box<dyn Radio>> {
    match a.backend {
        Backend::Sim => {
            let mut scene = Scene::new();
            for &[f, l] in &a.sim.tones {
                scene = scene.tone(f, l);
            }
            if a.sim.noise {
                scene = scene.adc_noise();
            }
            Ok(Box::new(SimRadio::new(SimConfig::hf(
                scene,
                Pacing::RealTime,
            ))))
        }
        Backend::Iq => open_iq(&a.iq),
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
            Ok(Box::new(ZynqsdrRadio::new(dev, cfg)?))
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

/// Reload the configuration when the file changes (checked every 2 s) or
/// on SIGHUP. Settings that apply at once go out through `tx`; the others
/// are logged as needing a restart. A broken file is logged and ignored.
fn watch_config(a: ServeArgs, mut running: Config, tx: tokio::sync::watch::Sender<Arc<Config>>) {
    // SAFETY: the handler only stores to an atomic
    unsafe { libc::signal(libc::SIGHUP, on_sighup as *const () as libc::sighandler_t) };
    let mtime = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let mut seen = mtime(&a.config);
    loop {
        std::thread::sleep(Duration::from_secs(2));
        let now = mtime(&a.config);
        let hup = RELOAD.swap(false, std::sync::atomic::Ordering::Relaxed);
        if now == seen && !hup {
            continue;
        }
        seen = now;
        let new = match a.effective() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("config: not reloaded: {e}");
                continue;
            }
        };
        let ch = running.changes(&new);
        if !ch.restart.is_empty() {
            eprintln!(
                "config: changes to [{}] take effect after a restart",
                ch.restart.join("], [")
            );
        }
        if ch.live {
            eprintln!("config: reloaded");
            let _ = tx.send(Arc::new(new.clone()));
        }
        running = new;
    }
}

fn run_serve(a: ServeArgs) -> AnyResult<()> {
    use web888_hpsdr::{proto::Board, server};
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
    let (live_tx, live_rx) = tokio::sync::watch::channel(Arc::new(cfg.clone()));
    {
        let (a, c) = (a.clone(), cfg.clone());
        std::thread::Builder::new()
            .name("config".into())
            .spawn(move || watch_config(a, c, live_tx))?;
    }

    let mut radio = open_radio(&cfg.radio)?;
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
    let web_rx = if !cfg.web.enabled {
        None
    } else {
        let ch = cfg.web.rx_channel.unwrap_or(caps.rx_channels - 1);
        if ch >= caps.rx_channels {
            return Err(format!(
                "web rx channel {ch}: the radio has {} channels",
                caps.rx_channels
            )
            .into());
        }
        Some(ch)
    };

    let mut threads = Vec::new();
    if cfg.hpsdr.enabled {
        let h = &cfg.hpsdr;
        let (sub, blk_rx, _drops) = web888_hw::Subscriber::new(64);
        ctl_tx.send(web888_hw::HwCmd::SubscribeRx(sub))?;
        let sock = UdpSocket::bind(&h.listen).map_err(|e| format!("bind {}: {e}", h.listen))?;
        // receivers map to channels 0..max_rx: keep the web user's out
        let max_rx = h
            .receivers
            .min(caps.rx_channels)
            .min(web_rx.unwrap_or(usize::MAX))
            .min(7);
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
        let ctl = ctl_tx.clone();
        threads.push(
            std::thread::Builder::new()
                .name("hpsdr".into())
                .spawn(move || {
                    srv.run(
                        blk_rx,
                        move |c| {
                            let cmd = match c {
                                server::Control::Tune { rx, hz } => {
                                    web888_hw::HwCmd::Tune { ch: rx, hz }
                                }
                                server::Control::Atten { db } => web888_hw::HwCmd::Atten { db },
                                server::Control::Streaming(_) => return,
                            };
                            let _ = ctl.send(cmd);
                        },
                        srv_stop,
                    )
                })?,
        );
    }

    if let Some(ch) = web_rx {
        let w = &cfg.web;
        let rf = caps.rf_range.clone();
        let shared = Arc::new(web888_web::session::Shared {
            hw: std::sync::Mutex::new(ctl_tx.clone()),
            status: status.clone(),
            radio: web888_web::proto::RadioInfo {
                backend: id.backend.into(),
                variant: format!("{:?}", caps.variant).to_lowercase(),
                adc_hz: caps.adc_hz,
                rx_rate: rx_rate(caps.adc_hz, r2),
                rf_lo: *rf.start(),
                rf_hi: *rf.end(),
                wf_decim_min: caps.wf_decim_min,
                wf_decim_max: caps.wf_decim_max,
            },
            rx_channel: ch,
            rx_r2: r2,
            live: live_rx,
            busy: Default::default(),
            generation: Default::default(),
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
            "web: {}://{} (RX channel {ch}, UI from {})",
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
