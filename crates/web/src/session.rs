// SPDX-License-Identifier: GPL-3.0-or-later
//! One web user: a WebSocket connection, an RX channel lease, its audio
//! job on the DSP pool, and its waterfall: drawn from its own channel when
//! the view lies inside it, else a (shared) hardware view.

use crate::proto::{self, ClientMsg, Limits, Mode, RadioInfo, ServerMsg, State, UserMark, WfReq};
use crate::users::{UserPub, Users};
use crate::wfsvc::{HwView, Waterfalls};
use crate::workers::{AudioCmd, AudioJob, ChanView, Out, WfSink};
use axum::extract::ws::{CloseFrame, Message, Utf8Bytes, WebSocket};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;
use web888_core::leases::{Channels, Class, Lease, LeaseState};
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
    pub users: Users,
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

    pub fn hello(&self, session: SessionId) -> ServerMsg {
        ServerMsg::Hello {
            session,
            class: "web".into(),
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
        self.sh.users.leave(self.id);
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

fn lease_msg(s: LeaseState) -> ServerMsg {
    match s {
        LeaseState::Granted { .. } => ServerMsg::Lease {
            granted: true,
            position: 0,
        },
        LeaseState::Waiting { position } => ServerMsg::Lease {
            granted: false,
            position,
        },
        LeaseState::Closed => ServerMsg::Lease {
            granted: false,
            position: 0,
        },
    }
}

/// The next publication of the user followed (none: they left); never
/// ready when not following.
async fn followed(f: &mut Option<(SessionId, watch::Receiver<UserPub>)>) -> Option<UserPub> {
    match f {
        Some((_, rx)) => match rx.changed().await {
            Ok(()) => Some(*rx.borrow_and_update()),
            Err(_) => None,
        },
        None => std::future::pending().await,
    }
}

/// A session's state between messages.
struct Session<'a> {
    sh: &'a Shared,
    id: SessionId,
    st: State,
    /// The user's own dial (`st.hz` follows the user listened along to).
    own_hz: f64,
    lease: Lease,
    follow: Option<(SessionId, watch::Receiver<UserPub>)>,
    me: watch::Sender<UserPub>,
    audio: crossbeam_channel::Sender<AudioCmd>,
    sink: WfSink,
}

impl Session<'_> {
    /// The channel the audio comes from: the followed user's or our own.
    fn channel(&self) -> Option<usize> {
        match &self.follow {
            Some((_, rx)) => rx.borrow().channel,
            None => self.lease.channel(),
        }
    }

    fn publish(&self) {
        let p = UserPub {
            hz: self.own_hz,
            mode: self.st.mode,
            channel: self.lease.channel(),
            share: self.st.share,
        };
        self.me.send_if_modified(|v| {
            let changed = *v != p;
            *v = p;
            changed
        });
    }

    /// Point the audio and the waterfall at the current channel and dial.
    fn route(&self) {
        let ch = self.channel();
        let _ = self.audio.send(AudioCmd::Channel(ch));
        apply_wf(
            self.sh,
            self.id,
            &self.st,
            ch.is_some(),
            &self.sink,
            &self.audio,
        );
    }

    fn listen(&mut self, user: SessionId) -> Result<(), String> {
        if self.lease.channel().is_some() {
            return Err("you have a receiver: tune to their frequency instead".into());
        }
        let rx = (user != self.id)
            .then(|| self.sh.users.follow(user))
            .flatten()
            .filter(|rx| rx.borrow().followable())
            .ok_or(format!("user {user} can't be listened to"))?;
        self.st.hz = rx.borrow().hz;
        self.follow = Some((user, rx));
        self.st.listen = Some(user);
        self.route();
        Ok(())
    }

    fn stop_listening(&mut self) {
        if self.follow.take().is_some() {
            self.st.listen = None;
            self.st.hz = self.own_hz;
            self.route();
        }
    }

    /// The followed user changed (`None`: left). Returns a notice when
    /// listening along ended.
    fn on_followed(&mut self, p: Option<UserPub>) -> Option<String> {
        let user = self.follow.as_ref()?.0;
        match p {
            Some(p) if p.followable() => {
                // their dial, and maybe their channel, moved
                self.st.hz = p.hz;
                self.route();
                None
            }
            Some(p) if !p.share => {
                self.stop_listening();
                Some(format!("user {user} stopped sharing"))
            }
            Some(_) => {
                self.stop_listening();
                Some(format!("user {user} lost their receiver"))
            }
            None => {
                self.stop_listening();
                Some(format!("user {user} left"))
            }
        }
    }

    fn handle(&mut self, msg: ClientMsg) -> Vec<ServerMsg> {
        let st = |s: &Self| ServerMsg::State(s.st.clone());
        match msg {
            ClientMsg::Hello { proto, .. } if proto != crate::proto::PROTO => {
                vec![ServerMsg::Error {
                    msg: format!(
                        "protocol {proto} not supported (server: {})",
                        crate::proto::PROTO
                    ),
                }]
            }
            ClientMsg::Hello { .. } => vec![],
            ClientMsg::Ping { t } => vec![ServerMsg::Pong { t }],
            ClientMsg::Tune { hz } => {
                // tuning ends listening along
                self.follow = None;
                self.st.listen = None;
                self.own_hz = self.sh.clamp_hz(hz);
                self.st.hz = self.own_hz;
                self.lease.tune(self.own_hz);
                self.publish();
                self.route();
                vec![st(self)]
            }
            ClientMsg::Mode {
                mode,
                lo,
                hi,
                pitch,
            } => {
                let d = AudioSpec::new(mode.into());
                self.st.mode = mode;
                self.st.lo = lo.unwrap_or(d.lo_hz);
                self.st.hi = hi.unwrap_or(d.hi_hz);
                self.st.pitch = pitch.unwrap_or(self.st.pitch);
                let a = audio_spec(&self.st);
                sync_state(&mut self.st, &a);
                let _ = self.audio.send(AudioCmd::Spec(a));
                self.publish();
                vec![st(self)]
            }
            ClientMsg::Audio { agc, gain_db } => {
                self.st.agc = agc.unwrap_or(self.st.agc);
                self.st.gain_db = gain_db.unwrap_or(self.st.gain_db);
                let a = audio_spec(&self.st);
                sync_state(&mut self.st, &a);
                let _ = self.audio.send(AudioCmd::Spec(a));
                vec![st(self)]
            }
            ClientMsg::Wf(w) => {
                self.st.wf = self.sh.clamp_wf(w);
                self.route();
                vec![st(self)]
            }
            ClientMsg::Share { on } => {
                self.st.share = on;
                self.publish();
                vec![st(self)]
            }
            ClientMsg::Listen { user: Some(u) } => match self.listen(u) {
                Ok(()) => vec![st(self)],
                Err(msg) => vec![ServerMsg::Error { msg }],
            },
            ClientMsg::Listen { user: None } => {
                self.stop_listening();
                vec![st(self)]
            }
        }
    }
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
    let hz = sh.default_hz();
    let mut st = State {
        hz,
        mode,
        lo: defaults.lo_hz,
        hi: defaults.hi_hz,
        pitch: defaults.pitch_hz,
        agc: true,
        gain_db: 100.0,
        wf: sh.full_view(hz),
        share: sh.live.borrow().limits.share_default,
        listen: None,
    };
    let spec = audio_spec(&st);
    sync_state(&mut st, &spec);
    let lease = sh
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
    let me = sh.users.join(
        id,
        UserPub {
            hz: st.hz,
            mode: st.mode,
            channel: lease.channel(),
            share: st.share,
        },
    );
    let mut s = Session {
        sh: &sh,
        id,
        own_hz: st.hz,
        st,
        lease,
        follow: None,
        me,
        audio: audio_cmd,
        sink,
    };
    s.route();

    if !send_json(&mut ws, &sh.hello(id)).await
        || !send_json(&mut ws, &ServerMsg::State(s.st.clone())).await
        || !send_json(&mut ws, &lease_msg(s.lease.state())).await
    {
        return;
    }

    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut users_sent: Option<Vec<UserMark>> = None;
    loop {
        let replies = tokio::select! {
            m = ws.recv() => {
                let Some(Ok(m)) = m else { break };
                let text = match m {
                    Message::Text(t) => t,
                    Message::Close(_) => break,
                    _ => continue,
                };
                match serde_json::from_str::<ClientMsg>(&text) {
                    Ok(m) => s.handle(m),
                    Err(e) => vec![ServerMsg::Error { msg: format!("bad message: {e}") }],
                }
            }
            l = s.lease.changed() => {
                s.publish();
                if s.follow.is_none() {
                    s.route();
                }
                vec![lease_msg(l)]
            }
            p = followed(&mut s.follow) => {
                let mut r = vec![];
                if let Some(msg) = s.on_followed(p) {
                    r.push(ServerMsg::Error { msg });
                }
                r.push(ServerMsg::State(s.st.clone()));
                r
            }
            Some(f) = audio_rx.recv() => {
                if ws.send(Message::Binary(f.into())).await.is_err() {
                    break;
                }
                continue;
            }
            Some(f) = wf_rx.recv() => {
                if ws.send(Message::Binary(f.into())).await.is_err() {
                    break;
                }
                continue;
            }
            _ = tick.tick() => {
                let hw = &sh.status;
                let mut r = vec![ServerMsg::Status {
                    adc_overloads: hw.overload_blocks.load(Ordering::Relaxed),
                    clock_ppm: hw.clock_ppb.load(Ordering::Relaxed) as f64 / 1e3,
                    audio_drops: audio_drops.load(Ordering::Relaxed)
                        + placed.rx_dropped.load(Ordering::Relaxed).saturating_sub(rx_lost0),
                    wf_drops: wf_drops.load(Ordering::Relaxed),
                    hw_rate_ok: hw.rx_rate_ok.load(Ordering::Relaxed),
                }];
                let users = sh.users.marks(id);
                if users_sent.as_ref() != Some(&users) {
                    users_sent = Some(users.clone());
                    r.push(ServerMsg::Users { users });
                }
                r
            }
        };
        for r in &replies {
            if !send_json(&mut ws, r).await {
                return;
            }
        }
    }
    // dropping the command sender ends the audio job; the admission
    // takes the session off its waterfall view and the users list
}
