// SPDX-License-Identifier: GPL-3.0-or-later
//! web888d (M1).
//!
//! `serve`: run the radio (simulated, or the stock driver on a unit) and
//! offer its channels over HPSDR Protocol 1 (UDP 1024):
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

mod hw;

use clap::{Parser, Subcommand, ValueEnum};
use std::io::Write;
use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::time::{Duration, Instant};
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
    /// Run the radio and serve its channels (HPSDR Protocol 1).
    Serve(ServeArgs),
}

#[derive(Clone, Copy, ValueEnum)]
enum Backend {
    /// Simulated radio (real-time).
    Sim,
    /// Stock kernel driver /dev/zynqsdr (on the unit; stop the stock server first).
    Zynqsdr,
    /// Real signals: wideband IQ from an SDR or a recording (--iq ...),
    /// channelized like the FPGA.
    Iq,
}

#[derive(clap::Args)]
struct ServeArgs {
    #[arg(long, value_enum, default_value = "zynqsdr")]
    backend: Backend,
    /// Bitstream to load first (zynqsdr backend): a path, `auto` for the
    /// variant's file in /usr/share/web888 (web888-ng-os image) or else the
    /// stock one on the SD card, or `none` if it's already loaded.
    #[arg(long, default_value = "auto")]
    bitstream: String,
    /// EXPERIMENTAL (test unit only): reprogram the Si5351 ADC clock to this
    /// frequency before loading the bitstream. Default: keep the FSBL setting.
    #[arg(long)]
    si5351_adc: Option<f64>,
    /// HPSDR Protocol 1 listen address.
    #[arg(long, default_value = "0.0.0.0:1024")]
    hpsdr: String,
    /// Board id reported to HPSDR clients (1 = Hermes, 5 = Orion, 6 = Hermes-Lite).
    #[arg(long, default_value_t = 1)]
    hpsdr_board: u8,
    /// Receivers offered to HPSDR clients (max 7).
    #[arg(long, default_value_t = 7)]
    hpsdr_rx: usize,
    /// Simulated scene (sim backend): tones FREQ_HZ:DBFS.
    #[arg(long = "tone", value_parser = parse_tone)]
    tones: Vec<(f64, f64)>,
    #[arg(long)]
    noise: bool,
    /// IQ backend input: `file:PATH` (.cs8 as from hackrf_transfer, .cs16,
    /// .cf32) or `soapy:ARGS` (e.g. soapy:driver=hackrf).
    #[arg(long)]
    iq: Option<String>,
    /// IQ input centre frequency (Hz); channels work within +-0.45 x rate.
    #[arg(long)]
    iq_freq: Option<f64>,
    /// IQ input sample rate: 122.88 MHz / 8, 16, ..., 256.
    #[arg(long, default_value_t = 7.68e6)]
    iq_rate: f64,
    /// Device gain element NAME=DB (soapy; repeatable), e.g. LNA=16, VGA=20, AMP=0.
    #[arg(long = "iq-gain", value_parser = parse_gain)]
    iq_gains: Vec<(String, f64)>,
    /// Device analog bandwidth in Hz (soapy; default: the driver's choice).
    #[arg(long, default_value_t = 0.0)]
    iq_bw: f64,
    /// Device antenna port (soapy).
    #[arg(long)]
    iq_antenna: Option<String>,
    /// Start a file again at its end.
    #[arg(long)]
    iq_loop: bool,
    #[arg(short, long)]
    verbose: bool,
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

fn open_iq(a: &ServeArgs) -> AnyResult<Box<dyn Radio>> {
    use web888_iq::source::{FileSource, Format, IqSource};
    use web888_iq::{IqConfig, IqRadio};
    let spec =
        a.iq.as_deref()
            .ok_or("--backend iq needs --iq file:PATH or --iq soapy:ARGS")?;
    let fc = a
        .iq_freq
        .ok_or("--backend iq needs --iq-freq (input centre frequency)")?;
    let cfg = IqConfig::new(fc, a.iq_rate);
    let src: Box<dyn IqSource> = if let Some(p) = spec.strip_prefix("file:") {
        let path = std::path::Path::new(p);
        let fmt = Format::from_path(path).ok_or("IQ file: use extension .cs8, .cs16 or .cf32")?;
        Box::new(FileSource::open(path, fmt, a.iq_loop).map_err(|e| format!("{p}: {e}"))?)
    } else if let Some(args) = spec.strip_prefix("soapy:") {
        open_soapy(args, fc, a)?
    } else {
        return Err("--iq: expected file:PATH or soapy:ARGS".into());
    };
    eprintln!(
        "iq: {spec}, {:.3} MS/s around {:.6} MHz (channels {:.3}..{:.3} MHz)",
        a.iq_rate / 1e6,
        fc / 1e6,
        (fc - 0.45 * a.iq_rate) / 1e6,
        (fc + 0.45 * a.iq_rate) / 1e6
    );
    Ok(Box::new(IqRadio::new(cfg, src)?))
}

#[cfg(feature = "soapy")]
fn open_soapy(
    args: &str,
    fc: f64,
    a: &ServeArgs,
) -> AnyResult<Box<dyn web888_iq::source::IqSource>> {
    use web888_iq::source::{SoapyConfig, SoapySource};
    let c = SoapyConfig {
        args: args.to_string(),
        center_hz: fc,
        rate_hz: a.iq_rate,
        bandwidth_hz: a.iq_bw,
        gains: a.iq_gains.clone(),
        antenna: a.iq_antenna.clone(),
    };
    Ok(Box::new(
        SoapySource::open(&c).map_err(|e| format!("soapy:{args}: {e}"))?,
    ))
}

#[cfg(not(feature = "soapy"))]
fn open_soapy(_: &str, _: f64, _: &ServeArgs) -> AnyResult<Box<dyn web888_iq::source::IqSource>> {
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
            if let Err(e) = run_serve(a) {
                eprintln!("web888d: {e}");
                std::process::exit(1);
            }
        }
    }
}

type AnyResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn open_radio(a: &ServeArgs) -> AnyResult<Box<dyn Radio>> {
    match a.backend {
        Backend::Sim => {
            let mut scene = Scene::new();
            for &(f, l) in &a.tones {
                scene = scene.tone(f, l);
            }
            if a.noise {
                scene = scene.adc_noise();
            }
            Ok(Box::new(SimRadio::new(SimConfig::hf(
                scene,
                Pacing::RealTime,
            ))))
        }
        Backend::Iq => open_iq(a),
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

fn run_serve(a: ServeArgs) -> AnyResult<()> {
    use web888_hpsdr::{proto::Board, server};
    let mut radio = open_radio(&a)?;
    let caps = radio.caps();
    let id = radio.identity();
    eprintln!(
        "radio: {} signature {:#010x}, {} RX / {} WF channels, ADC {:.0} Hz",
        id.backend, id.signature, caps.rx_channels, caps.wf_channels, caps.adc_hz
    );
    // HPSDR P1 is served at 48 kS/s
    radio.set_rx_rate(r2_for_rate(caps.adc_hz, 48_000.0))?;

    let stop = Arc::new(AtomicBool::new(false));
    let (ctl_tx, ctl_rx) = mpsc::channel();
    let (sub, blk_rx, _hpsdr_drops) = hw::Subscriber::new(64);
    ctl_tx.send(hw::HwCmd::SubscribeRx(sub))?;

    let sock = UdpSocket::bind(&a.hpsdr).map_err(|e| format!("bind {}: {e}", a.hpsdr))?;
    let cfg = server::Config {
        board: Board {
            mac: eth0_mac(),
            code_version: 1,
            board_id: a.hpsdr_board,
        },
        max_rx: a.hpsdr_rx.min(caps.rx_channels).min(7),
    };
    let mut srv = server::Server::new(sock, cfg)?;
    eprintln!(
        "hpsdr: listening on {} ({} receivers)",
        srv.local_addr()?,
        cfg.max_rx
    );
    let srv_stop = stop.clone();
    let srv_thread = std::thread::Builder::new()
        .name("hpsdr".into())
        .spawn(move || {
            srv.run(
                blk_rx,
                move |c| {
                    let cmd = match c {
                        server::Control::Tune { rx, hz } => hw::HwCmd::Tune { ch: rx, hz },
                        server::Control::Atten { db } => hw::HwCmd::Atten { db },
                        server::Control::Streaming(_) => return,
                    };
                    let _ = ctl_tx.send(cmd);
                },
                srv_stop,
            )
        })?;

    let hw = hw::HwLoop {
        radio,
        ctl: ctl_rx,
        stop: stop.clone(),
        verbose: a.verbose,
    };
    let res = hw.run();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    srv_thread.join().ok();
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
