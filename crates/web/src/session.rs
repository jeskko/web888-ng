// SPDX-License-Identifier: GPL-3.0-or-later
//! One web user: a WebSocket connection, an RX channel lease, its audio
//! job on the DSP pool, and its waterfall: drawn from its own channel when
//! the view lies inside it, else a (shared) hardware view.

use crate::proto::{self, ClientMsg, Limits, Mode, RadioInfo, ServerMsg, State, WfReq};
use crate::wfsvc::{HwView, Waterfalls};
use crate::workers::{AudioCmd, AudioJob, ChanView, Out, WfSink};
use axum::extract::ws::{CloseFrame, Message, Utf8Bytes, WebSocket};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use web888_core::leases::{Channels, Class};
use web888_core::pool::DspPool;
use web888_core::wf::SessionId;
use web888_dsp::agc::AgcSpec;
use web888_dsp::audio::{AUDIO_RATE, AudioSpec};
use web888_dsp::chan::Droop;
use web888_hw::HwStatus;

pub const BINS_MAX: usize = 4096;
pub const FPS_MAX: f64 = 30.0;
/// Views at most this wide, inside the RX channel's flat part around the
/// dial, are drawn from the channel (no WF slot).
const CHAN_VIEW_SPAN: f64 = 40_000.0;
const CHAN_VIEW_EDGE: f64 = 20_000.0;

/// What every session shares: the hw thread's status, the radio
/// description, the RX channel leases, the DSP pool and the waterfalls.
pub struct Shared {
    pub status: Arc<HwStatus>,
    pub radio: RadioInfo,
    pub channels: Arc<Channels>,
    pub pool: Arc<DspPool>,
    pub waterfalls: Waterfalls,
    /// Stage-2 RX decimation the hardware runs at (droop compensation).
    pub rx_r2: u16,
    /// The configuration's live settings (reloaded without a restart).
    pub live: tokio::sync::watch::Receiver<Arc<web888_config::Config>>,
    /// Sessions per client address.
    pub ips: Mutex<HashMap<IpAddr, usize>>,
    pub sessions: AtomicUsize,
    pub next_session: AtomicU64,
}

impl Shared {
    /// Initial dial frequency: the configured one, else 7.074 MHz if in
    /// range, else the middle of the range.
    pub fn default_hz(&self) -> f64 {
        let r = &self.radio;
        let want = self.live.borrow().web.default_freq.unwrap_or(
            if (r.rf_lo..=r.rf_hi).contains(&7_074_000.0) {
                7_074_000.0
            } else {
                (r.rf_lo + r.rf_hi) / 2.0
            },
        );
        want.clamp(r.rf_lo, r.rf_hi)
    }

    /// FFTs per waterfall line at most.
    pub fn wf_budget(&self) -> usize {
        self.live.borrow().web.wf_budget.max(1)
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
            self.default_hz()
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

/// A session's place: counted per client address; its waterfall is
/// released when dropped.
struct Admission<'a> {
    sh: &'a Shared,
    ip: IpAddr,
    id: SessionId,
}

impl<'a> Admission<'a> {
    /// None if the address has its limit of sessions already.
    fn new(sh: &'a Shared, ip: IpAddr) -> Result<Self, usize> {
        let limit = sh.live.borrow().limits.per_ip;
        let mut ips = sh.ips.lock().unwrap();
        let n = ips.entry(ip).or_insert(0);
        if limit > 0 && *n >= limit {
            return Err(limit);
        }
        *n += 1;
        sh.sessions.fetch_add(1, Ordering::Relaxed);
        Ok(Admission {
            sh,
            ip,
            id: sh.next_session.fetch_add(1, Ordering::Relaxed) + 1,
        })
    }
}

impl Drop for Admission<'_> {
    fn drop(&mut self) {
        self.sh.waterfalls.remove(self.id);
        let mut ips = self.sh.ips.lock().unwrap();
        if let Some(n) = ips.get_mut(&self.ip) {
            *n -= 1;
            if *n == 0 {
                ips.remove(&self.ip);
            }
        }
        self.sh.sessions.fetch_sub(1, Ordering::Relaxed);
    }
}

/// The session's view drawn from its channel, if it fits there.
fn chan_view(sh: &Shared, st: &State, granted: bool) -> Option<ChanView> {
    let span = sh.radio.adc_hz / st.wf.decim as f64;
    let off = (st.wf.center_hz - st.hz).abs() + span / 2.0;
    (granted && span <= CHAN_VIEW_SPAN && off <= CHAN_VIEW_EDGE).then_some(ChanView {
        dial_hz: st.hz,
        center_hz: st.wf.center_hz,
        span_hz: span,
        bins: st.wf.bins,
        fps: st.wf.fps,
        budget: sh.wf_budget(),
    })
}

/// Point the session's waterfall at its view: from its channel or from
/// the hardware.
fn apply_wf(
    sh: &Shared,
    id: SessionId,
    st: &State,
    granted: bool,
    sink: &WfSink,
    audio: &crossbeam_channel::Sender<AudioCmd>,
) {
    let cv = chan_view(sh, st, granted);
    let _ = audio.send(AudioCmd::Wf(cv));
    let hw = cv.is_none().then_some(HwView {
        center_hz: st.wf.center_hz,
        decim: st.wf.decim,
        bins: st.wf.bins,
        fps: st.wf.fps,
    });
    sh.waterfalls.set(id, hw, sink, sh.wf_budget());
}

async fn send_json(ws: &mut WebSocket, m: &ServerMsg) -> bool {
    let s = serde_json::to_string(m).expect("serialisable");
    ws.send(Message::Text(s.into())).await.is_ok()
}

pub async fn run(mut ws: WebSocket, sh: Arc<Shared>, peer: std::net::SocketAddr) {
    let adm = match Admission::new(&sh, peer.ip()) {
        Ok(a) => a,
        Err(limit) => {
            let _ = send_json(
                &mut ws,
                &ServerMsg::Busy {
                    msg: format!("too many sessions from your address (at most {limit})"),
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
    };
    let id = adm.id;

    let mode = Mode::Usb;
    let defaults = AudioSpec::new(mode.into());
    let mut st = State {
        hz: sh.default_hz(),
        mode,
        lo: defaults.lo_hz,
        hi: defaults.hi_hz,
        pitch: defaults.pitch_hz,
        agc: true,
        gain_db: 100.0,
        wf: sh.full_view(sh.default_hz()),
    };
    let spec = audio_spec(&st);
    sync_state(&mut st, &spec);
    let mut lease = sh
        .channels
        .acquire(Class::Web, &format!("web {}", peer.ip()), st.hz);

    // DSP: the channel (audio, narrow waterfalls) on the pool; wider
    // waterfalls from the shared hardware views
    let (audio_tx, mut audio_rx) = tokio::sync::mpsc::channel(32);
    let (wf_tx, mut wf_rx) = tokio::sync::mpsc::channel(16);
    let audio_drops = Arc::new(AtomicU64::new(0));
    let wf_drops = Arc::new(AtomicU64::new(0));
    let sink = WfSink::new(Out {
        tx: wf_tx,
        drops: wf_drops.clone(),
    });
    let (audio_cmd, audio_cmd_rx) = crossbeam_channel::unbounded();
    let job = AudioJob::new(
        lease.channel(),
        sh.radio.rx_rate,
        Droop::Rx {
            adc_hz: sh.radio.adc_hz,
            r2: sh.rx_r2,
        },
        spec,
        audio_cmd_rx,
        Out {
            tx: audio_tx,
            drops: audio_drops.clone(),
        },
        sink.clone(),
    );
    let placed = sh.pool.add(Box::new(job));
    // the worker's losses count from here
    let rx_lost0 = placed.rx_dropped.load(Ordering::Relaxed);
    apply_wf(&sh, id, &st, lease.channel().is_some(), &sink, &audio_cmd);

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
                        lease.tune(st.hz);
                        apply_wf(&sh, id, &st, lease.channel().is_some(), &sink, &audio_cmd);
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
                        let _ = audio_cmd.send(AudioCmd::Spec(a));
                        Some(ServerMsg::State(st.clone()))
                    }
                    ClientMsg::Audio { agc, gain_db } => {
                        st.agc = agc.unwrap_or(st.agc);
                        st.gain_db = gain_db.unwrap_or(st.gain_db);
                        let a = audio_spec(&st);
                        sync_state(&mut st, &a);
                        let _ = audio_cmd.send(AudioCmd::Spec(a));
                        Some(ServerMsg::State(st.clone()))
                    }
                    ClientMsg::Wf(w) => {
                        st.wf = sh.clamp_wf(w);
                        apply_wf(&sh, id, &st, lease.channel().is_some(), &sink, &audio_cmd);
                        Some(ServerMsg::State(st.clone()))
                    }
                };
                if let Some(r) = reply && !send_json(&mut ws, &r).await {
                    break;
                }
            }
            s = lease.changed() => {
                // M3 step 5 tells the client (waiting / granted)
                let _ = audio_cmd.send(AudioCmd::Channel(s.channel()));
                apply_wf(&sh, id, &st, s.channel().is_some(), &sink, &audio_cmd);
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
                    audio_drops: audio_drops.load(Ordering::Relaxed)
                        + placed.rx_dropped.load(Ordering::Relaxed).saturating_sub(rx_lost0),
                    wf_drops: wf_drops.load(Ordering::Relaxed),
                    hw_rate_ok: s.rx_rate_ok.load(Ordering::Relaxed),
                };
                if !send_json(&mut ws, &m).await {
                    break;
                }
            }
        }
    }
    // dropping the command sender ends the audio job; the admission
    // takes the session off its waterfall view
}
