// SPDX-License-Identifier: GPL-3.0-or-later
//! web888d (M1).
//!
//! `serve`: run the radio (simulated, or the stock driver on a unit) and
//! offer its channels over HPSDR Protocol 1 (UDP 1024):
//!
//!   web888d serve --backend zynqsdr          # on the unit, stock server stopped
//!   web888d serve --backend sim --tone 7.0755e6:-40 --noise
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
}

#[derive(clap::Args)]
struct ServeArgs {
    #[arg(long, value_enum, default_value = "zynqsdr")]
    backend: Backend,
    /// Bitstream to load first (zynqsdr backend): a path, `auto` for the
    /// stock file on the SD card, or `none` if it's already loaded.
    #[arg(long, default_value = "auto")]
    bitstream: String,
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
        Backend::Zynqsdr => {
            use web888_zynqsdr::{ZynqConfig, ZynqsdrRadio, bitstream, dev::ZynqsdrDev};
            let cfg = ZynqConfig::default();
            let bit = match a.bitstream.as_str() {
                "none" => None,
                "auto" => Some(bitstream::default_path(cfg.variant)),
                p => Some(std::path::PathBuf::from(p)),
            };
            if let Some(bit) = bit {
                let n = bitstream::load(&bit, std::path::Path::new(bitstream::XDEVCFG))
                    .map_err(|e| format!("loading {}: {e}", bit.display()))?;
                eprintln!("bitstream: {} ({n} bytes)", bit.display());
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
    let (blk_tx, blk_rx) = mpsc::sync_channel(64);
    let (ctl_tx, ctl_rx) = mpsc::channel();

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
        .spawn(move || srv.run(blk_rx, ctl_tx, srv_stop))?;

    let hw = hw::HwLoop {
        radio,
        ctl: ctl_rx,
        out: blk_tx,
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
