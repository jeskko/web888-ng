// SPDX-License-Identifier: GPL-3.0-or-later
//! A Kiwi audio stream (`/<n>/SND`, `doc/m5-kiwi.md` §3): an RX channel
//! lease (class web, as a listener; admin with the admin password), its audio
//! job packing Kiwi `SND` packets, and the client's `SET` commands.

use super::wire::{Cmd, SND_FLAG_LPF, SND_FLAG_NEW_FREQ, SND_FLAG_RESTART, SndEnc, SndSet};
use super::{Geometry, answer_auth, band_msgs, command, send_config, send_msg, wait_auth};
use crate::auth::Role;
use crate::proto;
use crate::session::{Admission, Shared, Timeout, Timeouts};
use crate::users::{Name, UserPub};
use crate::workers::{AudioCmd, AudioJob, Framing, Out, WfSink};
use axum::extract::ws::{CloseFrame, Message, Utf8Bytes, WebSocket};
use axum::extract::{ConnectInfo, Path, RawQuery, State, WebSocketUpgrade};
use axum::response::{IntoResponse, Response};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::{Duration, Instant};
use web888_core::leases::Class;
use web888_dsp::agc::{AgcSpec, AgcSpeed};
use web888_dsp::audio::{AudioSpec, Mode};
use web888_dsp::chan::Droop;

pub(crate) async fn upgrade(
    up: WebSocketUpgrade,
    State(sh): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(ts): Path<String>,
    RawQuery(q): RawQuery,
) -> Response {
    let Some(ts) = super::timestamp(&ts).filter(|_| sh.live.borrow().kiwi.enabled) else {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    };
    let camp = q.is_some_and(|q| q.split('&').any(|p| p == "camp"));
    up.on_upgrade(move |ws| run(ws, sh, peer, ts, camp))
}

/// A Kiwi mode name: web888d's demodulator, the mode others see, and the
/// passband if the client gives none. Kiwi's CW is USB around +500 Hz
/// (its passband says where); the modes web888d lacks fall back to AM, as
/// upstream does with unknown modes.
fn mode(name: &str) -> (Mode, proto::Mode, (f64, f64)) {
    match name {
        "usb" | "usn" => (Mode::Usb, proto::Mode::Usb, (300.0, 2700.0)),
        "lsb" | "lsn" => (Mode::Lsb, proto::Mode::Lsb, (-2700.0, -300.0)),
        "cw" | "cwn" => (Mode::Usb, proto::Mode::Cw, (300.0, 700.0)),
        "sam" => (Mode::Sam, proto::Mode::Sam, (-4900.0, 4900.0)),
        "sal" => (Mode::Sal, proto::Mode::Sal, (-4900.0, 50.0)),
        "sau" => (Mode::Sar, proto::Mode::Sar, (-50.0, 4900.0)),
        "sas" => (Mode::Sas, proto::Mode::Sas, (-4900.0, 4900.0)),
        "iq" | "drm" => (Mode::Iq, proto::Mode::Iq, (-5000.0, 5000.0)),
        "nbfm" | "nnfm" => (Mode::Nbfm, proto::Mode::Nbfm, (-5000.0, 5000.0)),
        _ => (Mode::Am, proto::Mode::Am, (-4900.0, 4900.0)),
    }
}

/// What the client has set, and what has to come before audio flows (as
/// upstream: frequency and mode, AGC, and the client's audio).
struct Rx {
    hz: f64,
    mode: Mode,
    pmode: proto::Mode,
    lo: f64,
    hi: f64,
    agc: bool,
    agc_speed: AgcSpeed,
    gain_db: f32,
    nb: bool,
    tuned: bool,
    agc_set: bool,
    ar_ok: bool,
}

impl Rx {
    fn ready(&self) -> bool {
        self.tuned && self.agc_set && self.ar_ok
    }

    fn spec(&self) -> AudioSpec {
        let mut a = AudioSpec::new(self.mode);
        a.lo_hz = self.lo;
        a.hi_hz = self.hi;
        a.nb = self.nb.then_some(8.0);
        if self.agc {
            let base = if self.mode.is_am() && self.mode != Mode::Iq {
                AgcSpec::am()
            } else {
                AgcSpec::ssb_at(self.agc_speed)
            };
            a.agc = Some(base);
            a.gain_db = base.max_gain_db;
        } else {
            a.agc = None;
            a.gain_db = self.gain_db;
        }
        a.clamped()
    }
}

async fn close(ws: &mut WebSocket, code: u16, reason: &'static str) {
    let _ = ws
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: Utf8Bytes::from_static(reason),
        })))
        .await;
}

async fn run(mut ws: WebSocket, sh: Arc<Shared>, peer: SocketAddr, ts: u64, camp: bool) {
    let ip = peer.ip();
    let role = match wait_auth(&mut ws, &sh, ip).await {
        None => return,
        Some(Err(badp)) => {
            let _ = answer_auth(&mut ws, &sh, badp).await;
            return close(&mut ws, 1008, "bad password").await;
        }
        Some(Ok(r)) => r,
    };
    if camp {
        // camping on another stream isn't offered
        let _ = answer_auth(&mut ws, &sh, super::BADP_OK).await;
        let _ = send_msg(&mut ws, "camp_disconnect").await;
        return close(&mut ws, 1000, "no camping").await;
    }
    // a listener like a web user (not under HPSDR's protocol quota)
    let class = if role == Role::Admin {
        Class::Admin
    } else {
        Class::Web
    };
    let busy = |sh: &Shared| format!("too_busy={}", sh.channels.channels());
    let Some(entry) = sh.kiwi.add_snd(ip, ts) else {
        // the same stream twice
        let _ = answer_auth(&mut ws, &sh, super::BADP_NO_MULTIPLE_CONNS).await;
        return close(&mut ws, 1008, "duplicate").await;
    };
    let Ok(adm) = Admission::new(&sh, ip, class == Class::Admin) else {
        let _ = answer_auth(&mut ws, &sh, super::BADP_NO_MULTIPLE_CONNS).await;
        return close(&mut ws, 1013, "busy").await;
    };
    let id = adm.id;
    let g = Geometry::new(&sh.radio);
    let hz = sh.default_hz();
    let mut lease = sh.channels.acquire(class, &format!("kiwi {ip}"), hz);
    let Some(ch) = lease.channel() else {
        // Kiwi has no queue: all channels taken
        let _ = answer_auth(&mut ws, &sh, super::BADP_OK).await;
        let _ = send_msg(&mut ws, &busy(&sh)).await;
        return close(&mut ws, 1013, "busy").await;
    };
    let live = sh.live.borrow().clone();
    if !answer_auth(&mut ws, &sh, super::BADP_OK).await
        || !send_msg(&mut ws, &format!("is_local={ch},0")).await
        || !send_config(&mut ws, &sh).await
    {
        return;
    }
    for t in band_msgs(&sh) {
        if !send_msg(&mut ws, &t).await {
            return;
        }
    }
    let rate = super::sample_rate(&sh);
    if !send_msg(
        &mut ws,
        &format!(
            "audio_init=0 audio_rate={:.0} sample_rate={rate:.6}",
            web888_dsp::audio::AUDIO_RATE
        ),
    )
    .await
    {
        return;
    }

    let (m, pm, (lo, hi)) = mode("am");
    let mut rx = Rx {
        hz,
        mode: m,
        pmode: pm,
        lo,
        hi,
        agc: true,
        agc_speed: AgcSpeed::Medium,
        gain_db: 50.0,
        nb: false,
        tuned: false,
        agc_set: false,
        ar_ok: false,
    };
    // the audio job: no channel until the client is ready
    let (audio_tx, mut audio_rx) = tokio::sync::mpsc::channel(32);
    let (wf_tx, _wf_rx) = tokio::sync::mpsc::channel(1);
    let audio_drops = Arc::new(AtomicU64::new(0));
    let (audio, audio_cmd_rx) = crossbeam_channel::unbounded();
    let job = AudioJob::new(
        None,
        sh.radio.rx_rate,
        if sh.waterfalls.bins_mode() {
            Droop::None
        } else {
            Droop::Rx {
                adc_hz: sh.radio.adc_hz,
                r2: sh.rx_r2,
            }
        },
        rx.spec(),
        audio_cmd_rx,
        Out {
            tx: audio_tx,
            drops: audio_drops.clone(),
        },
        WfSink::new(Out {
            tx: wf_tx,
            drops: Arc::default(),
        }),
    )
    .with_framing(Framing::Kiwi(Box::new(SndEnc::new(
        entry.seq.clone(),
        live.kiwi.smeter_offset_db,
    ))));
    sh.pool.add(Box::new(job));
    let share = live.limits.share_default;
    let mut name = String::new();
    let me = sh.users.join(
        id,
        UserPub {
            name: Name::default(),
            hz,
            mode: rx.pmode,
            channel: Some(ch),
            share,
        },
    );
    let publish = |rx: &Rx, name: &str, ch: Option<usize>| {
        let p = UserPub {
            name: Name::new(name),
            hz: rx.hz,
            mode: rx.pmode,
            channel: ch,
            share,
        };
        me.send_if_modified(|v| {
            let changed = *v != p;
            *v = p;
            changed
        });
    };
    let kick = Arc::new(tokio::sync::Notify::new());
    sh.admin.sessions.lock().unwrap().insert(
        id,
        crate::admin::Entry {
            ip,
            class,
            since: std::time::SystemTime::now(),
            lease: lease.id(),
            kick: kick.clone(),
            listen: Arc::default(),
            audio_drops,
            wf_drops: Arc::default(),
        },
    );

    let started = Instant::now();
    let mut last_msg = started;
    let mut timeouts = Timeouts::default();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            m = ws.recv() => {
                let Some(Ok(m)) = m else { break };
                if matches!(m, Message::Close(_)) {
                    break;
                }
                let Some(text) = command(&m) else { continue };
                last_msg = Instant::now();
                let was_ready = rx.ready();
                let mut set = SndSet::default();
                let mut spec = false;
                let cmd = Cmd::parse(&text);
                if !matches!(cmd, Cmd::Keepalive | Cmd::Auth { .. }) {
                    timeouts.input();
                }
                match cmd {
                    Cmd::Tune { mode: mname, lo, hi, khz } => {
                        let f = g.rf_lo + khz * 1e3;
                        if f.is_finite() {
                            let f = f.clamp(sh.radio.rf_lo, sh.radio.rf_hi);
                            if f != rx.hz || !rx.tuned {
                                set.flags |= SND_FLAG_NEW_FREQ;
                                rx.hz = f;
                                lease.tune(f);
                            }
                        }
                        let keep_pb = lo == 0.0 && hi == 0.0;
                        if let Some(n) = mname {
                            let (m, pm, pb) = mode(&n);
                            if m != rx.mode || pm != rx.pmode {
                                set.flags |= SND_FLAG_NEW_FREQ;
                                rx.mode = m;
                                rx.pmode = pm;
                                if keep_pb {
                                    (rx.lo, rx.hi) = pb;
                                    set.flags |= SND_FLAG_LPF;
                                }
                            }
                        }
                        if !keep_pb && (lo, hi) != (rx.lo, rx.hi) && lo.is_finite() && hi.is_finite() {
                            (rx.lo, rx.hi) = (lo.min(hi), lo.max(hi));
                            set.flags |= SND_FLAG_LPF;
                        }
                        rx.tuned = true;
                        spec = true;
                        publish(&rx, &name, lease.channel());
                    }
                    Cmd::Agc { on, decay_ms, gain_db, .. } => {
                        rx.agc = on;
                        rx.agc_speed = match decay_ms {
                            ..=300 => AgcSpeed::Fast,
                            301..=1500 => AgcSpeed::Medium,
                            _ => AgcSpeed::Slow,
                        };
                        rx.gain_db = gain_db as f32;
                        rx.agc_set = true;
                        spec = true;
                    }
                    Cmd::Nb(on) => {
                        rx.nb = on;
                        spec = true;
                    }
                    Cmd::ArOk => rx.ar_ok = true,
                    Cmd::Compression(c) => set.compression = Some(c),
                    Cmd::LittleEndian => set.little_endian = true,
                    Cmd::Reinit => set.flags |= SND_FLAG_RESTART,
                    Cmd::Ident(n) => {
                        name = n;
                        publish(&rx, &name, lease.channel());
                    }
                    // the rest web888d doesn't do (upstream ignores the
                    // commands it lacks too)
                    _ => {}
                }
                if spec {
                    let _ = audio.send(AudioCmd::Spec(rx.spec()));
                }
                if set != SndSet::default() {
                    let _ = audio.send(AudioCmd::Kiwi(set));
                }
                if rx.ready() && !was_ready {
                    let _ = audio.send(AudioCmd::Channel(lease.channel()));
                }
            }
            l = lease.changed() => {
                let Some(c) = l.channel() else {
                    // taken by a higher class: Kiwi clients don't wait
                    let _ = send_msg(&mut ws, &busy(&sh)).await;
                    close(&mut ws, 1013, "busy").await;
                    break;
                };
                if rx.ready() {
                    let _ = audio.send(AudioCmd::Channel(Some(c)));
                }
                publish(&rx, &name, Some(c));
            }
            Some(f) = audio_rx.recv() => {
                if ws.send(Message::Binary(f.into())).await.is_err() {
                    break;
                }
            }
            _ = kick.notified() => {
                close(&mut ws, 1008, "kicked").await;
                break;
            }
            _ = tick.tick() => {
                if last_msg.elapsed() > super::KEEPALIVE {
                    close(&mut ws, 1001, "no keepalive").await;
                    break;
                }
                if class == Class::Admin {
                    continue;
                }
                // the session time limit; the idle limit is for people
                // (a recorder never tunes)
                let l = sh.live.borrow().limits.clone();
                if let Some(Timeout::End(code, _)) =
                    timeouts.check(&l, sh.limit_unit, started.elapsed(), Duration::ZERO)
                {
                    let _ = send_msg(&mut ws, "inactivity_timeout=1").await;
                    close(&mut ws, code, "time limit").await;
                    break;
                }
            }
        }
    }
    drop(audio);
    drop(entry);
    // the admission takes the session off the users list and the registry
    drop(adm);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kiwi_modes_map_to_web888d_demodulators() {
        assert_eq!(mode("usb").0, Mode::Usb);
        assert_eq!(mode("cwn"), (Mode::Usb, proto::Mode::Cw, (300.0, 700.0)));
        assert_eq!(mode("sau").0, Mode::Sar);
        assert_eq!(mode("iq").0, Mode::Iq);
        assert_eq!(mode("drm").0, Mode::Iq);
        assert_eq!(mode("nbfm").0, Mode::Nbfm);
        assert_eq!(mode("nnfm").0, Mode::Nbfm);
        assert_eq!(mode("whatever").0, Mode::Am);
    }

    #[test]
    fn audio_waits_for_frequency_agc_and_the_client() {
        let (m, pm, (lo, hi)) = mode("lsb");
        let mut rx = Rx {
            hz: 7e6,
            mode: m,
            pmode: pm,
            lo,
            hi,
            agc: false,
            agc_speed: AgcSpeed::Medium,
            gain_db: 30.0,
            nb: true,
            tuned: true,
            agc_set: true,
            ar_ok: false,
        };
        assert!(!rx.ready());
        rx.ar_ok = true;
        assert!(rx.ready());
        let s = rx.spec();
        assert_eq!((s.lo_hz, s.hi_hz), (-2700.0, -300.0));
        assert_eq!(s.agc, None);
        assert_eq!(s.gain_db, 30.0);
        assert_eq!(s.nb, Some(8.0));
    }
}
