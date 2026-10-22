// SPDX-License-Identifier: GPL-3.0-or-later
//! A session's DSP threads: audio (one RX channel to PCM frames) and
//! waterfall (WF slot samples to lines). They take blocks from the hw
//! fan-out and hand encoded frames to the session's WebSocket task; a full
//! queue drops the frame (counted), it never blocks.

use crate::proto::{self, FLAG_GAP, FLAG_NEW_GEN, FLAG_OVERLOAD};
use crossbeam_channel::{Receiver as CbReceiver, TryRecvError};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::error::TrySendError;
use web888_dsp::audio::{AUDIO_RATE, AudioChain, AudioSpec};
use web888_dsp::chan::Droop;
use web888_dsp::wf::{WF_DB_STEP, WF_DB0, WfEngine, WfSpec, quantize};
use web888_radio::{RxBlock, WfBlock};

/// Audio frame length: 20 ms at 12 kS/s.
pub const AUDIO_FRAME: usize = 240;

/// Frames to the WebSocket task, with a drop counter.
#[derive(Clone)]
pub struct Out {
    pub tx: Sender<Vec<u8>>,
    pub drops: Arc<AtomicU64>,
}

impl Out {
    /// False when the session is gone.
    fn send(&self, frame: Vec<u8>) -> bool {
        match self.tx.try_send(frame) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                self.drops.fetch_add(1, Ordering::Relaxed);
                true
            }
            Err(TrySendError::Closed(_)) => false,
        }
    }
}

/// Changes for the audio thread.
#[derive(Clone, Copy, Debug)]
pub enum AudioCmd {
    Spec(AudioSpec),
    /// The RX channel carrying the session's lease (none: no audio).
    Channel(Option<usize>),
}

pub struct AudioWorker {
    pub ch: Option<usize>,
    pub rx_rate: f64,
    pub droop: Droop,
    pub spec: AudioSpec,
    pub blocks: Receiver<Arc<RxBlock>>,
    pub cmd: CbReceiver<AudioCmd>,
    pub out: Out,
}

impl AudioWorker {
    pub fn run(self) {
        let mut chain = AudioChain::new(self.spec, self.rx_rate, self.droop);
        let mut pcm: Vec<f32> = Vec::new();
        let mut next_frame = None;
        let mut flags = 0u8;
        let mut seq = 0u32;
        let mut rssi = -200.0;
        let mut ch = self.ch;
        loop {
            loop {
                match self.cmd.try_recv() {
                    Ok(AudioCmd::Spec(s)) => chain.set_spec(s),
                    Ok(AudioCmd::Channel(c)) if c != ch => {
                        // another channel's history: start over
                        ch = c;
                        chain = AudioChain::new(*chain.spec(), self.rx_rate, self.droop);
                        pcm.clear();
                        flags |= FLAG_GAP;
                    }
                    Ok(AudioCmd::Channel(_)) => {}
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                }
            }
            let b = match self.blocks.recv_timeout(Duration::from_millis(100)) {
                Ok(b) => b,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return,
            };
            let Some(ch) = ch else {
                next_frame = None;
                continue;
            };
            if next_frame.is_some_and(|n| n != b.first_frame) {
                flags |= FLAG_GAP;
            }
            next_frame = Some(b.end_frame());
            if b.overload {
                flags |= FLAG_OVERLOAD;
            }
            chain.process(b.channel(ch), &mut pcm);
            let mut used = 0;
            while pcm.len() - used >= AUDIO_FRAME {
                // a block may finish several frames: they share its reading
                rssi = chain.take_power_db().unwrap_or(rssi);
                let f = proto::audio_frame(
                    flags,
                    seq,
                    AUDIO_RATE as u32,
                    rssi,
                    &pcm[used..used + AUDIO_FRAME],
                );
                if !self.out.send(f) {
                    return;
                }
                flags = 0;
                seq = seq.wrapping_add(1);
                used += AUDIO_FRAME;
            }
            pcm.drain(..used);
        }
    }
}

/// A waterfall configuration from the session.
#[derive(Clone, Copy, Debug)]
pub struct WfConfig {
    pub generation: u64,
    pub center_hz: f64,
    pub spec: WfSpec,
}

pub struct WfWorker {
    pub blocks: Receiver<Arc<WfBlock>>,
    pub cmd: CbReceiver<WfConfig>,
    pub out: Out,
}

fn unix_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1e3)
}

impl WfWorker {
    pub fn run(self) {
        let mut cfg: Option<WfConfig> = None;
        let mut engine: Option<WfEngine> = None;
        let mut prev: Option<Arc<WfBlock>> = None;
        let mut period = Duration::from_millis(50);
        let mut deadline = Instant::now() + period;
        let mut new_gen = false;
        let mut seq = 0u32;
        loop {
            loop {
                match self.cmd.try_recv() {
                    Ok(c) => {
                        engine = Some(WfEngine::new(c.spec));
                        period = Duration::from_secs_f64(1.0 / c.spec.fps);
                        deadline = Instant::now() + period;
                        cfg = Some(c);
                        prev = None;
                        new_gen = true;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                }
            }
            let wait = deadline.saturating_duration_since(Instant::now());
            match self
                .blocks
                .recv_timeout(wait.min(Duration::from_millis(100)))
            {
                Ok(b) => {
                    if let (Some(c), Some(e)) = (&cfg, &mut engine)
                        && b.generation == c.generation
                    {
                        let contiguous = prev.as_ref().is_some_and(|p| b.continues(p));
                        e.push(&b.iq, contiguous);
                        prev = Some(b);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            let now = Instant::now();
            if now < deadline {
                continue;
            }
            deadline += period;
            if deadline < now {
                deadline = now + period; // fell behind: don't burst
            }
            let (Some(c), Some(e)) = (&cfg, &mut engine) else {
                continue;
            };
            let Some(line) = e.line() else { continue };
            let flags = if std::mem::take(&mut new_gen) {
                FLAG_NEW_GEN
            } else {
                0
            };
            let f = proto::wf_frame(
                flags,
                seq,
                c.center_hz + line.first_offset_hz,
                line.bin_hz,
                WF_DB0,
                WF_DB_STEP,
                unix_ms(),
                &quantize(&line.db),
            );
            seq = seq.wrapping_add(1);
            if !self.out.send(f) {
                return;
            }
        }
    }
}
