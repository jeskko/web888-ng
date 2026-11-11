// SPDX-License-Identifier: GPL-3.0-or-later
//! A Kiwi waterfall stream (`/<n>/W/F`, `doc/m5-kiwi.md` §4): zoom and
//! start become a shared hardware view (the narrowest one covering the
//! span), its lines are resampled to Kiwi's 1024 pixels. With its SND
//! stream (same address and timestamp) it is part of that session and
//! tags its lines with the audio's sequence; alone (kiwirecorder `--wf`)
//! it is a session of its own, without an RX channel.

use super::wire::{Cmd, MAX_ZOOM, WF_SPEEDS, WF_WIDTH, WfEnc, WfView};
use super::{Geometry, answer_auth, band_msgs, command, send_config, send_msg, wait_auth};
use crate::session::{Admission, BINS_MAX, Shared};
use crate::wfsvc::HwView;
use crate::workers::{Out, WfSink};
use axum::extract::ws::{CloseFrame, Message, Utf8Bytes, WebSocket};
use axum::extract::{ConnectInfo, Path, State, WebSocketUpgrade};
use axum::response::{IntoResponse, Response};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use web888_core::wf::SessionId;

pub(crate) async fn upgrade(
    up: WebSocketUpgrade,
    State(sh): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(ts): Path<String>,
) -> Response {
    let Some(ts) = super::timestamp(&ts).filter(|_| sh.live.borrow().kiwi.enabled) else {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    };
    up.on_upgrade(move |ws| run(ws, sh, peer, ts))
}

/// A paired stream's waterfall, taken off when it ends (a stream of its
/// own has its admission for that).
struct ViewGuard<'a> {
    sh: &'a Shared,
    id: SessionId,
}

impl Drop for ViewGuard<'_> {
    fn drop(&mut self) {
        self.sh.waterfalls.remove(self.id);
    }
}

/// The hardware view for a Kiwi view: the narrowest span the hardware has
/// that still covers it, enough bins for 1024 pixels of it.
pub(crate) fn hw_view(sh: &Shared, v: &WfView, fps: u32) -> HwView {
    let r = &sh.radio;
    let mut decim = r.wf_decim_min.max(1);
    while decim < r.wf_decim_max && r.adc_hz / (2.0 * decim as f64) >= v.span_hz * 0.999 {
        decim *= 2;
    }
    let hw_span = r.adc_hz / decim as f64;
    let bins = ((WF_WIDTH as f64 * hw_span / v.span_hz).ceil() as usize)
        .next_power_of_two()
        .clamp(256, BINS_MAX);
    let w = sh.clamp_wf(crate::proto::WfReq {
        center_hz: v.start_hz + v.span_hz / 2.0,
        decim,
        bins,
        fps: fps as f64,
    });
    HwView {
        center_hz: w.center_hz,
        decim: w.decim,
        bins: w.bins,
        fps: w.fps,
    }
}

/// Zoom (clamped) and start (in the protocol's bins, clamped so the view
/// stays in the band), from `start` or a centre `cf_khz` (baseband kHz).
pub(crate) fn place(
    g: &Geometry,
    zoom: u32,
    cf_khz: Option<f64>,
    start: Option<f64>,
) -> (u32, u32) {
    let zoom = zoom.min(MAX_ZOOM);
    let span = g.span(zoom);
    let hz = g.hz_per_start();
    let s = match (cf_khz, start) {
        (Some(cf), _) => (cf * 1e3 - span / 2.0) / hz,
        (None, Some(s)) => s,
        (None, None) => 0.0,
    };
    let max = ((g.bandwidth - span) / hz).max(0.0);
    let s = if s.is_finite() {
        s.clamp(0.0, max)
    } else {
        0.0
    };
    (zoom, s as u32)
}

async fn run(mut ws: WebSocket, sh: Arc<Shared>, peer: SocketAddr, ts: u64) {
    let ip = peer.ip();
    match wait_auth(&mut ws, &sh, ip).await {
        None => return,
        Some(Err(badp)) => {
            let _ = answer_auth(&mut ws, &sh, badp).await;
            let _ = ws
                .send(Message::Close(Some(CloseFrame {
                    code: 1008,
                    reason: Utf8Bytes::from_static("bad password"),
                })))
                .await;
            return;
        }
        Some(Ok(_)) => {}
    }
    // with its SND stream: part of that session; else one of its own
    let paired = sh.kiwi.snd_seq(ip, ts);
    let mut _adm = None;
    let mut _guard = None;
    let id = match &paired {
        Some(_) => {
            let id = sh.next_session.fetch_add(1, Ordering::Relaxed) + 1;
            _guard = Some(ViewGuard { sh: &sh, id });
            id
        }
        None => match Admission::new(&sh, ip, false) {
            Ok(a) => {
                let id = a.id;
                _adm = Some(a);
                id
            }
            Err(_) => {
                let _ = answer_auth(&mut ws, &sh, super::BADP_NO_MULTIPLE_CONNS).await;
                return;
            }
        },
    };
    let n = sh.channels.channels();
    // every user can have a waterfall (shared views)
    let wf_chans = n;
    if !answer_auth(&mut ws, &sh, super::BADP_OK).await || !send_config(&mut ws, &sh).await {
        return;
    }
    for t in band_msgs(&sh).into_iter().chain([
        "kiwi_up=1 rx_chan=0".to_string(),
        format!(
            "wf_fft_size=1024 wf_fps={f} wf_fps_max={f} zoom_max={MAX_ZOOM} rx_chans={n} \
             wf_chans={wf_chans} wf_chans_real={wf_chans} wf_cal=0 wf_setup",
            f = WF_SPEEDS[4]
        ),
    ]) {
        if !send_msg(&mut ws, &t).await {
            return;
        }
    }

    let g = Geometry::new(&sh.radio);
    let dbm_offset = sh.live.borrow().kiwi.smeter_offset_db;
    let enc = Arc::new(WfEnc {
        view: Mutex::new(WfView {
            start_hz: g.rf_lo,
            span_hz: g.bandwidth,
            x_bin: 0,
            zoom: 0,
            compression: false,
            dbm_offset,
        }),
        own_seq: paired.is_none(),
        seq: paired.unwrap_or_else(|| Arc::new(AtomicU32::new(0))),
    });
    let (tx, mut lines) = tokio::sync::mpsc::channel(16);
    let sink = WfSink {
        out: Out {
            tx,
            drops: Arc::new(AtomicU64::new(0)),
        },
        seq: Arc::default(),
        kiwi: Some(enc.clone()),
    };
    // a view needs a zoom and a speed (as upstream: all of them first)
    let mut placed = false;
    let mut fps: Option<u32> = None;
    let apply = |placed: bool, fps: Option<u32>| {
        let v = *enc.view.lock().unwrap();
        let view = match fps {
            Some(f) if f > 0 && placed => Some(hw_view(&sh, &v, f)),
            _ => None,
        };
        sh.waterfalls.set(id, view, &sink, sh.wf_budget());
    };

    let mut last_msg = Instant::now();
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
                match Cmd::parse(&text) {
                    Cmd::Zoom { zoom, cf_khz, start } => {
                        let (z, s) = place(&g, zoom, cf_khz, start);
                        {
                            let mut v = enc.view.lock().unwrap();
                            v.zoom = z;
                            v.x_bin = s;
                            v.span_hz = g.span(z);
                            v.start_hz = g.rf_lo + s as f64 * g.hz_per_start();
                        }
                        placed = true;
                        apply(placed, fps);
                        if !send_msg(&mut ws, &format!("zoom={z} start={s}")).await {
                            break;
                        }
                    }
                    Cmd::WfSpeed(sp) => {
                        let sp = if sp == -1 { WF_SPEEDS.len() as i32 - 1 } else { sp };
                        if let Some(&f) = usize::try_from(sp).ok().and_then(|i| WF_SPEEDS.get(i)) {
                            fps = Some(f);
                            apply(placed, fps);
                        }
                        let f = fps.unwrap_or(0);
                        if !send_msg(&mut ws, &format!("wf_fps={f}")).await {
                            break;
                        }
                    }
                    Cmd::WfComp(c) => enc.view.lock().unwrap().compression = c,
                    // aperture, colours, interpolation, the window: the
                    // client's or not done here
                    _ => {}
                }
            }
            Some(f) = lines.recv() => {
                if ws.send(Message::Binary(f.into())).await.is_err() {
                    break;
                }
            }
            _ = tick.tick() => {
                if last_msg.elapsed() > super::KEEPALIVE {
                    break;
                }
            }
        }
    }
    sh.waterfalls.set(id, None, &sink, 1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_and_centre_place_the_view_in_the_band() {
        let g = Geometry {
            rf_lo: 0.0,
            bandwidth: 61_440_000.0,
        };
        // zoom 0: the whole band, start 0 whatever the centre
        assert_eq!(place(&g, 0, Some(7000.0), None), (0, 0));
        // 3.84 MHz around 7.1 MHz: starts at 5.18 MHz
        let (z, s) = place(&g, 4, Some(7100.0), None);
        assert_eq!(z, 4);
        let start = s as f64 * g.hz_per_start();
        assert!((start - 5_180_000.0).abs() < 4.0, "{start}");
        // near the top: kept inside
        let (_, s) = place(&g, 4, Some(61_000.0), None);
        assert!((s as f64 * g.hz_per_start() + g.span(4) - g.bandwidth).abs() < 4.0);
        // the old API, and zooms past the deepest
        assert_eq!(place(&g, 20, None, Some(1e12)).0, MAX_ZOOM);
        assert_eq!(place(&g, 2, None, Some(-5.0)), (2, 0));
    }
}
