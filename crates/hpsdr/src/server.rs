// SPDX-License-Identifier: GPL-3.0-or-later
//! Protocol 1 server loop. Runs on its own thread with a blocking UDP
//! socket; takes sample blocks from the hardware thread over a channel and
//! hands control commands to its [`Host`], so it never touches the radio
//! directly.
//!
//! The host maps receivers to hardware channels (web888d: one lease per
//! receiver while the client streams); a receiver without a channel sends
//! zeros. A client that goes silent for [`CLIENT_TIMEOUT`] is stopped, so
//! it doesn't hold channels. Only 48 kS/s is supported (other speeds are
//! logged and served at 48k); RX blocks at another rate
//! ([`Config::rx_rate`], e.g. 60 kS/s from a channelizer's bins) are
//! resampled to it.

use crate::proto::*;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};
use web888_dsp::C32;
use web888_dsp::resample::Resampler;
use web888_radio::{Iq32, RxBlock};

/// The one sample rate served.
pub const RATE: f64 = 48_000.0;

/// A streaming client is stopped after this long without a packet
/// (clients send EP2 data hundreds of times a second while running).
pub const CLIENT_TIMEOUT: Duration = Duration::from_secs(3);

/// Commands for the hw thread.
#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    Tune {
        rx: usize,
        hz: f64,
    },
    Atten {
        db: f32,
    },
    /// Receivers in use: the client's count while it streams, 0 when it
    /// stops.
    Receivers(usize),
}

/// What the server asks of its owner.
pub trait Host {
    fn control(&mut self, c: Control);
    /// The hardware channel carrying receiver `rx` now (none: zeros).
    fn channel(&mut self, rx: usize) -> Option<usize> {
        Some(rx)
    }
}

/// A closure host: receivers map to channels `0..n` (tests, tools).
impl<F: FnMut(Control)> Host for F {
    fn control(&mut self, c: Control) {
        self(c)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub board: Board,
    /// Receivers offered (<= hardware channels, <= 7).
    pub max_rx: usize,
    /// The RX blocks' sample rate.
    pub rx_rate: f64,
}

/// L/M taking `from` to [`RATE`] (both whole Hz), if not 1.
fn ratio(from: f64) -> Option<(usize, usize)> {
    fn gcd(a: usize, b: usize) -> usize {
        if b == 0 { a } else { gcd(b, a % b) }
    }
    let (a, b) = (RATE.round() as usize, from.round() as usize);
    let g = gcd(a, b);
    (a != b && b > 0).then(|| (a / g, b / g))
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub packets_sent: u64,
}

pub struct Server {
    sock: UdpSocket,
    cfg: Config,
    state: HostState,
    client: Option<SocketAddr>,
    /// Last packet from the client.
    heard: Instant,
    streaming: bool,
    /// Last `Control::Receivers` reported.
    active: usize,
    ep6: Ep6Builder,
    /// The receivers' resampler (blocks not at 48 kS/s), its output.
    rs: Option<Resampler>,
    rs_in: Vec<C32>,
    rs_out: Vec<C32>,
    pub stats: Stats,
}

impl Server {
    pub fn new(sock: UdpSocket, cfg: Config) -> std::io::Result<Self> {
        sock.set_read_timeout(Some(Duration::from_millis(2)))?;
        Ok(Server {
            sock,
            cfg,
            state: HostState::default(),
            client: None,
            heard: Instant::now(),
            streaming: false,
            active: 0,
            ep6: Ep6Builder::new(1),
            rs: None,
            rs_in: Vec::new(),
            rs_out: Vec::new(),
            stats: Stats::default(),
        })
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.sock.local_addr()
    }

    /// Run until `stop` is set.
    pub fn run(
        &mut self,
        blocks: Receiver<Arc<RxBlock>>,
        mut host: impl Host,
        stop: Arc<AtomicBool>,
    ) {
        let mut buf = [0u8; 2048];
        while !stop.load(Ordering::Relaxed) {
            match self.sock.recv_from(&mut buf) {
                Ok((n, from)) => self.on_packet(&buf[..n], from, &mut host),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(e) => eprintln!("hpsdr: recv: {e}"),
            }
            loop {
                match blocks.try_recv() {
                    Ok(b) => self.on_block(&b, &mut host),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => break,
                }
            }
            if self.streaming && self.heard.elapsed() > CLIENT_TIMEOUT {
                eprintln!("hpsdr: client {:?} silent, stopped", self.client);
                self.streaming = false;
            }
            self.report(&mut host);
        }
        self.streaming = false;
        self.report(&mut host);
    }

    /// Tell the host when the receivers in use change.
    fn report(&mut self, host: &mut impl Host) {
        let n = if self.streaming { self.ep6.nrx() } else { 0 };
        if n != self.active {
            self.active = n;
            host.control(Control::Receivers(n));
        }
    }

    fn on_packet(&mut self, pkt: &[u8], from: SocketAddr, host: &mut impl Host) {
        if Some(from) == self.client {
            self.heard = Instant::now();
        }
        match parse(pkt) {
            Some(HostPacket::Discovery) => {
                let r = discovery_reply(&self.cfg.board, self.streaming, self.cfg.max_rx as u8);
                let _ = self.sock.send_to(&r, from);
            }
            Some(HostPacket::StartStop(cmd)) => {
                let on = cmd & 1 != 0;
                if on {
                    self.client = Some(from);
                    self.heard = Instant::now();
                    self.ep6 = Ep6Builder::new(self.state.nrx.min(self.cfg.max_rx));
                }
                self.streaming = on;
            }
            Some(HostPacket::Data { cc, .. }) => {
                for c in cc {
                    let old_nrx = self.state.nrx;
                    let old_speed = self.state.speed;
                    let old_att = self.state.atten_db;
                    for rx in self.state.apply(c) {
                        if rx < self.cfg.max_rx {
                            host.control(Control::Tune {
                                rx,
                                hz: self.state.rx_hz[rx] as f64,
                            });
                        }
                    }
                    if self.state.nrx != old_nrx {
                        self.ep6.set_nrx(self.state.nrx.min(self.cfg.max_rx));
                    }
                    if self.state.speed != old_speed && self.state.speed != 0 {
                        eprintln!(
                            "hpsdr: {} S/s requested, only 48000 is supported",
                            self.state.rate()
                        );
                    }
                    if self.state.atten_db != old_att {
                        host.control(Control::Atten {
                            db: self.state.atten_db.unwrap_or(0) as f32,
                        });
                    }
                }
            }
            None => {}
        }
    }

    fn on_block(&mut self, b: &RxBlock, host: &mut impl Host) {
        if !self.streaming {
            return;
        }
        let Some(to) = self.client else { return };
        let nrx = self.ep6.nrx();
        self.ep6.set_overload(b.overload);
        let map: Vec<Option<usize>> = (0..nrx)
            .map(|rx| host.channel(rx).filter(|&ch| ch < b.nch))
            .collect();
        let mut out = [Iq32::default(); 8];
        let Some((l, m)) = ratio(self.cfg.rx_rate) else {
            for frame in b.iq.chunks_exact(b.nch) {
                for (o, m) in out.iter_mut().zip(&map) {
                    *o = m.map_or(Iq32::default(), |ch| frame[ch]);
                }
                self.send(&out[..nrx], to);
            }
            return;
        };
        if self.rs.as_ref().is_none_or(|r| r.channels() != nrx) {
            self.rs = Some(Resampler::new(l, m, nrx));
        }
        let rs = self.rs.as_mut().expect("set");
        self.rs_out.clear();
        for frame in b.iq.chunks_exact(b.nch) {
            self.rs_in.clear();
            self.rs_in.extend(map.iter().map(|m| {
                m.map_or(C32::new(0.0, 0.0), |ch| {
                    C32::new(frame[ch].i as f32, frame[ch].q as f32)
                })
            }));
            rs.push(&self.rs_in, &mut self.rs_out);
        }
        let frames = std::mem::take(&mut self.rs_out);
        for f in frames.chunks_exact(nrx) {
            for (o, z) in out.iter_mut().zip(f) {
                *o = Iq32 {
                    i: z.re.round() as i32,
                    q: z.im.round() as i32,
                };
            }
            self.send(&out[..nrx], to);
        }
        self.rs_out = frames;
    }

    /// One frame of the receivers' samples to the client.
    fn send(&mut self, frame: &[Iq32], to: SocketAddr) {
        if let Some(p) = self.ep6.push(frame)
            && self.sock.send_to(&p, to).is_ok()
        {
            self.stats.packets_sent += 1;
        }
    }
}
