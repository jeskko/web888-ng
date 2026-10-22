// SPDX-License-Identifier: GPL-3.0-or-later
//! Protocol 1 server loop. Runs on its own thread with a blocking UDP
//! socket; takes sample blocks from the hardware thread over a channel and
//! hands control commands to a callback, so it never touches the radio
//! directly.
//!
//! Receivers map to hardware channels `0..nrx` (leases come in M3). Only
//! 48 kS/s is supported, so the hardware must run at 48 kS/s (r2 = 10 at
//! 122.88 MHz); other speeds are logged and served at 48k.

use crate::proto::*;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;
use web888_radio::RxBlock;

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
    /// Client started/stopped streaming.
    Streaming(bool),
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub board: Board,
    /// Receivers offered (<= hardware channels, <= 7).
    pub max_rx: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub packets_sent: u64,
    pub blocks_dropped: u64,
}

pub struct Server {
    sock: UdpSocket,
    cfg: Config,
    state: HostState,
    client: Option<SocketAddr>,
    streaming: bool,
    ep6: Ep6Builder,
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
            streaming: false,
            ep6: Ep6Builder::new(1),
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
        mut ctl: impl FnMut(Control),
        stop: Arc<AtomicBool>,
    ) {
        let mut buf = [0u8; 2048];
        while !stop.load(Ordering::Relaxed) {
            match self.sock.recv_from(&mut buf) {
                Ok((n, from)) => self.on_packet(&buf[..n], from, &mut ctl),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(e) => eprintln!("hpsdr: recv: {e}"),
            }
            loop {
                match blocks.try_recv() {
                    Ok(b) => self.on_block(&b),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                }
            }
        }
    }

    fn on_packet(&mut self, pkt: &[u8], from: SocketAddr, ctl: &mut impl FnMut(Control)) {
        match parse(pkt) {
            Some(HostPacket::Discovery) => {
                let r = discovery_reply(&self.cfg.board, self.streaming, self.cfg.max_rx as u8);
                let _ = self.sock.send_to(&r, from);
            }
            Some(HostPacket::StartStop(cmd)) => {
                let on = cmd & 1 != 0;
                if on {
                    self.client = Some(from);
                    self.ep6 = Ep6Builder::new(self.state.nrx.min(self.cfg.max_rx));
                }
                if on != self.streaming {
                    self.streaming = on;
                    ctl(Control::Streaming(on));
                }
            }
            Some(HostPacket::Data { cc, .. }) => {
                for c in cc {
                    let old_nrx = self.state.nrx;
                    let old_speed = self.state.speed;
                    let old_att = self.state.atten_db;
                    for rx in self.state.apply(c) {
                        if rx < self.cfg.max_rx {
                            ctl(Control::Tune {
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
                        ctl(Control::Atten {
                            db: self.state.atten_db.unwrap_or(0) as f32,
                        });
                    }
                }
            }
            None => {}
        }
    }

    fn on_block(&mut self, b: &RxBlock) {
        if !self.streaming {
            return;
        }
        let Some(to) = self.client else { return };
        let nrx = self.ep6.nrx();
        if b.nch < nrx {
            self.stats.blocks_dropped += 1;
            return;
        }
        self.ep6.set_overload(b.overload);
        for frame in b.iq.chunks_exact(b.nch) {
            if let Some(p) = self.ep6.push(&frame[..nrx])
                && self.sock.send_to(&p, to).is_ok()
            {
                self.stats.packets_sent += 1;
            }
        }
    }
}
