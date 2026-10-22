// SPDX-License-Identifier: GPL-3.0-or-later
//! One web user (M2: one at a time): a WebSocket connection, its RX
//! channel and WF slot 0, and its two DSP threads.

use crate::proto::{self, ClientMsg, Limits, Mode, RadioInfo, ServerMsg, State, WfReq};
use crate::workers::{AudioWorker, Out, WfConfig, WfWorker};
use axum::extract::ws::{CloseFrame, Message, Utf8Bytes, WebSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use web888_dsp::agc::AgcSpec;
use web888_dsp::audio::{AUDIO_RATE, AudioSpec};
use web888_dsp::chan::Droop;
use web888_dsp::wf::WfSpec;
use web888_hw::{HwCmd, HwStatus, Subscriber};

pub const BINS_MAX: usize = 4096;
pub const FPS_MAX: f64 = 30.0;
/// WF slot used by the web user.
const WF_SLOT: usize = 0;

/// What every session shares: the hw thread's command queue and status,
/// the radio description, and the one-user lock.
pub struct Shared {
    pub hw: Mutex<std::sync::mpsc::Sender<HwCmd>>,
    pub status: Arc<HwStatus>,
    pub radio: RadioInfo,
    /// RX channel for the web user.
    pub rx_channel: usize,
    /// Stage-2 RX decimation the hardware runs at (droop compensation).
    pub rx_r2: u16,
    /// Initial dial frequency.
    pub default_hz: f64,
    /// FFTs per waterfall line at most.
    pub wf_budget: usize,
    pub busy: AtomicBool,
    pub generation: AtomicU64,
}

impl Shared {
    fn hw(&self, c: HwCmd) {
        let _ = self.hw.lock().unwrap().send(c);
    }

    pub fn hello(&self) -> ServerMsg {
        ServerMsg::Hello {
            proto: proto::PROTO,
            server: format!("web888d {}", env!("CARGO_PKG_VERSION")),
            codec: "pcm16".into(),
            audio_rate: AUDIO_RATE,
            radio: self.radio.clone(),
            limits: Limits {
                bins_max: BINS_MAX,
                fps_max: FPS_MAX,
            },
        }
    }

    /// The widest waterfall: the whole tunable range, or if the widest
    /// span can't cover it (iq backend), centred on the dial `hz`.
    pub fn full_view(&self, hz: f64) -> WfReq {
        let r = &self.radio;
        let width = (r.rf_hi - r.rf_lo).max(1.0);
        let mut decim = r.wf_decim_max;
        while decim > r.wf_decim_min && r.adc_hz / (decim as f64) < width {
            decim /= 2;
        }
        let covers = r.adc_hz / (decim as f64) >= width;
        WfReq {
            center_hz: if covers {
                (r.rf_lo + r.rf_hi) / 2.0
            } else {
                hz
            },
            decim,
            bins: 1024,
            fps: 20.0,
        }
    }

    fn clamp_wf(&self, w: WfReq) -> WfReq {
        let r = &self.radio;
        let mut decim = r.wf_decim_min;
        while decim < r.wf_decim_max && decim < w.decim {
            decim *= 2;
        }
        WfReq {
            center_hz: w.center_hz.clamp(r.rf_lo, r.rf_hi),
            decim,
            bins: w.bins.clamp(256, BINS_MAX).next_power_of_two(),
            fps: if w.fps.is_finite() {
                w.fps.clamp(1.0, FPS_MAX)
            } else {
                20.0
            },
        }
    }

    fn clamp_hz(&self, hz: f64) -> f64 {
        if hz.is_finite() {
            hz.clamp(self.radio.rf_lo, self.radio.rf_hi)
        } else {
            self.default_hz
        }
    }
}

fn audio_spec(s: &State) -> AudioSpec {
    let mut a = AudioSpec::new(s.mode.into());
    a.lo_hz = s.lo;
    a.hi_hz = s.hi;
    a.pitch_hz = s.pitch;
    a.gain_db = s.gain_db;
    a.agc = if s.agc {
        let base = if s.mode == Mode::Am {
            AgcSpec::am()
        } else {
            AgcSpec::ssb()
        };
        Some(AgcSpec {
            max_gain_db: s.gain_db,
            ..base
        })
    } else {
        None
    };
    a.clamped()
}

/// Write the clamped spec back into the state the client sees.
fn sync_state(s: &mut State, a: &AudioSpec) {
    s.lo = a.lo_hz;
    s.hi = a.hi_hz;
    s.pitch = a.pitch_hz;
    s.gain_db = a.gain_db;
}

/// Releases the one-user lock and the WF slot when the session ends.
struct Lease<'a>(&'a Shared);

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        self.0.hw(HwCmd::WfStop { slot: WF_SLOT });
        self.0.busy.store(false, Ordering::Release);
    }
}

async fn send_json(ws: &mut WebSocket, m: &ServerMsg) -> bool {
    let s = serde_json::to_string(m).expect("serialisable");
    ws.send(Message::Text(s.into())).await.is_ok()
}

pub async fn run(mut ws: WebSocket, sh: Arc<Shared>) {
    if sh.busy.swap(true, Ordering::AcqRel) {
        let _ = send_json(
            &mut ws,
            &ServerMsg::Busy {
                msg: "another user has the receiver".into(),
            },
        )
        .await;
        let _ = ws
            .send(Message::Close(Some(CloseFrame {
                code: 1013,
                reason: Utf8Bytes::from_static("busy"),
            })))
            .await;
        return;
    }
    let _lease = Lease(&sh);

    let mode = Mode::Usb;
    let defaults = AudioSpec::new(mode.into());
    let mut st = State {
        hz: sh.default_hz,
        mode,
        lo: defaults.lo_hz,
        hi: defaults.hi_hz,
        pitch: defaults.pitch_hz,
        agc: true,
        gain_db: 100.0,
        wf: sh.full_view(sh.default_hz),
    };
    let spec = audio_spec(&st);
    sync_state(&mut st, &spec);

    // DSP threads
    let (audio_tx, mut audio_rx) = tokio::sync::mpsc::channel(32);
    let (wf_tx, mut wf_rx) = tokio::sync::mpsc::channel(16);
    let audio_drops = Arc::new(AtomicU64::new(0));
    let wf_drops = Arc::new(AtomicU64::new(0));
    let (rx_sub, rx_blocks, rx_lost) = Subscriber::new(64);
    let (wf_sub, wf_blocks, wf_lost) = Subscriber::new(256);
    let (audio_cmd, audio_cmd_rx) = crossbeam_channel::unbounded();
    let (wf_cmd, wf_cmd_rx) = crossbeam_channel::unbounded();
    let rx_rate = sh.radio.rx_rate;
    let worker = AudioWorker {
        ch: sh.rx_channel,
        rx_rate,
        droop: Droop::Rx {
            adc_hz: sh.radio.adc_hz,
            r2: sh.rx_r2,
        },
        spec,
        blocks: rx_blocks,
        cmd: audio_cmd_rx,
        out: Out {
            tx: audio_tx,
            drops: audio_drops.clone(),
        },
    };
    let spawned = std::thread::Builder::new()
        .name("web-audio".into())
        .spawn(move || worker.run())
        .and_then(|_| {
            let w = WfWorker {
                blocks: wf_blocks,
                cmd: wf_cmd_rx,
                out: Out {
                    tx: wf_tx,
                    drops: wf_drops.clone(),
                },
            };
            std::thread::Builder::new()
                .name("web-wf".into())
                .spawn(move || w.run())
        });
    if let Err(e) = spawned {
        let _ = send_json(&mut ws, &ServerMsg::Error { msg: e.to_string() }).await;
        return;
    }
    sh.hw(HwCmd::SubscribeRx(rx_sub));
    sh.hw(HwCmd::SubscribeWf(wf_sub));
    sh.hw(HwCmd::Tune {
        ch: sh.rx_channel,
        hz: st.hz,
    });
    let start_wf = |w: &WfReq| {
        let generation = sh.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let fs = sh.radio.adc_hz / w.decim as f64;
        let _ = wf_cmd.send(WfConfig {
            generation,
            center_hz: w.center_hz,
            spec: WfSpec {
                fs,
                bins: w.bins,
                budget: sh.wf_budget,
                fps: w.fps,
                droop: Some((sh.radio.adc_hz, w.decim)),
            },
        });
        sh.hw(HwCmd::WfStart {
            slot: WF_SLOT,
            hz: w.center_hz,
            decim: w.decim,
            generation,
        });
    };
    start_wf(&st.wf);

    if !send_json(&mut ws, &sh.hello()).await
        || !send_json(&mut ws, &ServerMsg::State(st.clone())).await
    {
        return;
    }

    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            m = ws.recv() => {
                let Some(Ok(m)) = m else { break };
                let text = match m {
                    Message::Text(t) => t,
                    Message::Close(_) => break,
                    _ => continue,
                };
                let msg = match serde_json::from_str::<ClientMsg>(&text) {
                    Ok(m) => m,
                    Err(e) => {
                        if !send_json(&mut ws, &ServerMsg::Error { msg: format!("bad message: {e}") }).await {
                            break;
                        }
                        continue;
                    }
                };
                let reply = match msg {
                    ClientMsg::Hello { proto, .. } if proto != crate::proto::PROTO => {
                        Some(ServerMsg::Error { msg: format!("protocol {proto} not supported (server: {})", crate::proto::PROTO) })
                    }
                    ClientMsg::Hello { .. } => None,
                    ClientMsg::Ping { t } => Some(ServerMsg::Pong { t }),
                    ClientMsg::Tune { hz } => {
                        st.hz = sh.clamp_hz(hz);
                        sh.hw(HwCmd::Tune { ch: sh.rx_channel, hz: st.hz });
                        Some(ServerMsg::State(st.clone()))
                    }
                    ClientMsg::Mode { mode, lo, hi, pitch } => {
                        let d = AudioSpec::new(mode.into());
                        st.mode = mode;
                        st.lo = lo.unwrap_or(d.lo_hz);
                        st.hi = hi.unwrap_or(d.hi_hz);
                        st.pitch = pitch.unwrap_or(st.pitch);
                        let a = audio_spec(&st);
                        sync_state(&mut st, &a);
                        let _ = audio_cmd.send(a);
                        Some(ServerMsg::State(st.clone()))
                    }
                    ClientMsg::Audio { agc, gain_db } => {
                        st.agc = agc.unwrap_or(st.agc);
                        st.gain_db = gain_db.unwrap_or(st.gain_db);
                        let a = audio_spec(&st);
                        sync_state(&mut st, &a);
                        let _ = audio_cmd.send(a);
                        Some(ServerMsg::State(st.clone()))
                    }
                    ClientMsg::Wf(w) => {
                        st.wf = sh.clamp_wf(w);
                        start_wf(&st.wf);
                        Some(ServerMsg::State(st.clone()))
                    }
                };
                if let Some(r) = reply && !send_json(&mut ws, &r).await {
                    break;
                }
            }
            Some(f) = audio_rx.recv() => {
                if ws.send(Message::Binary(f.into())).await.is_err() {
                    break;
                }
            }
            Some(f) = wf_rx.recv() => {
                if ws.send(Message::Binary(f.into())).await.is_err() {
                    break;
                }
            }
            _ = tick.tick() => {
                let s = &sh.status;
                let m = ServerMsg::Status {
                    adc_overloads: s.overload_blocks.load(Ordering::Relaxed),
                    clock_ppm: s.clock_ppb.load(Ordering::Relaxed) as f64 / 1e3,
                    audio_drops: audio_drops.load(Ordering::Relaxed) + rx_lost.load(Ordering::Relaxed),
                    wf_drops: wf_drops.load(Ordering::Relaxed) + wf_lost.load(Ordering::Relaxed),
                    hw_rate_ok: s.rx_rate_ok.load(Ordering::Relaxed),
                };
                if !send_json(&mut ws, &m).await {
                    break;
                }
            }
        }
    }
    // dropping the command senders stops the DSP threads
}
