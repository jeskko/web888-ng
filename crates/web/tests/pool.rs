// SPDX-License-Identifier: GPL-3.0-or-later
//! 13 sessions' audio on the DSP pool (M3 step 3): simulated radio -> hw
//! loop -> 2 pool workers -> 13 audio jobs on leases. 12 get a channel
//! and hear their own tone without gaps; the 13th waits, and hears its
//! tone once a channel frees.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use web888_core::leases::{Channels, Class, Lease, LeaseState, Policy};
use web888_core::pool::DspPool;
use web888_dsp::audio::{AudioSpec, Mode};
use web888_dsp::chan::Droop;
use web888_hw::{HwLoop, HwStatus};
use web888_radio::*;
use web888_sim::*;
use web888_web::proto::{AUDIO_HEADER, FLAG_GAP};
use web888_web::workers::{AudioCmd, AudioJob, Out, WfSink};

const SESSIONS: usize = 13;

/// Session k listens (USB) at `dial(k)`; its tone is `pitch(k)` above.
fn dial(k: usize) -> f64 {
    7_000_000.0 + 50_000.0 * k as f64
}

fn pitch(k: usize) -> f64 {
    600.0 + 100.0 * k as f64
}

struct Session {
    lease: Lease,
    cmd: crossbeam_channel::Sender<AudioCmd>,
    frames: mpsc::Receiver<Vec<u8>>,
    drops: Arc<AtomicU64>,
    audio: Vec<f32>,
    gaps: usize,
}

impl Session {
    /// Take the frames that arrived; true if any did.
    fn drain(&mut self) -> bool {
        let mut any = false;
        while let Ok(f) = self.frames.try_recv() {
            any = true;
            if f[1] & FLAG_GAP != 0 {
                self.gaps += 1;
            }
            self.audio.extend(
                f[AUDIO_HEADER..]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| i16::from_le_bytes(*c) as f32 / 32768.0),
            );
        }
        any
    }
}

/// Frequency of the strongest component of real audio (Hann + FFT).
fn dominant_hz(x: &[f32], fs: f64) -> f64 {
    let n = x.len().next_power_of_two() / 2;
    let mut buf: Vec<num_complex::Complex32> = x[x.len() - n..]
        .iter()
        .enumerate()
        .map(|(k, &v)| {
            let w = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * k as f32 / n as f32).cos();
            num_complex::Complex32::new(v * w, 0.0)
        })
        .collect();
    rustfft::FftPlanner::new()
        .plan_fft_forward(n)
        .process(&mut buf);
    let k = (1..n / 2)
        .max_by(|&a, &b| buf[a].norm().total_cmp(&buf[b].norm()))
        .unwrap();
    k as f64 * fs / n as f64
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thirteen_sessions_on_the_pool() {
    let mut scene = Scene::new().adc_noise();
    for k in 0..SESSIONS {
        scene = scene.tone(dial(k) + pitch(k), -50.0);
    }
    let mut radio = SimRadio::new(SimConfig::hf(scene, Pacing::RealTime));
    let caps = radio.caps();
    assert_eq!(caps.rx_channels, 12);
    let r2 = r2_for_rate(caps.adc_hz, 48_000.0);
    radio.set_rx_rate(r2).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let hw = HwLoop {
        radio: Box::new(radio),
        ctl: rx,
        stop: stop.clone(),
        status: Arc::new(HwStatus::default()),
        verbose: false,
    };
    let hw = std::thread::spawn(move || hw.run());
    let channels = Channels::new(caps.rx_channels, Policy::default(), tx.clone());
    let pool = DspPool::new(2, &tx, false).unwrap();
    let droop = Droop::Rx {
        adc_hz: caps.adc_hz,
        r2,
    };

    let mut sessions: Vec<Session> = (0..SESSIONS)
        .map(|k| {
            let lease = channels.acquire(Class::Web, &format!("s{k}"), dial(k));
            let (cmd, cmd_rx) = crossbeam_channel::unbounded();
            let (out, frames) = mpsc::channel(256);
            let drops = Arc::new(AtomicU64::new(0));
            let job = AudioJob::new(
                lease.channel(),
                rx_rate(caps.adc_hz, r2),
                droop,
                AudioSpec::new(Mode::Usb),
                cmd_rx,
                Out {
                    tx: out,
                    drops: drops.clone(),
                },
                WfSink::new(Out {
                    tx: mpsc::channel(1).0,
                    drops: Arc::default(),
                }),
            );
            pool.add(Box::new(job));
            Session {
                lease,
                cmd,
                frames,
                drops,
                audio: Vec::new(),
                gaps: 0,
            }
        })
        .collect();
    assert_eq!(
        sessions[12].lease.state(),
        LeaseState::Waiting { position: 1 }
    );
    let jobs: Vec<usize> = pool.stats().iter().map(|s| s.jobs).collect();
    assert_eq!(jobs, [7, 6]);

    // 3 s of audio
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(3) {
        tokio::time::sleep(Duration::from_millis(20)).await;
        for s in &mut sessions {
            s.drain();
        }
    }
    let secs = t0.elapsed().as_secs_f64();
    let stats = pool.stats();
    for (w, st) in stats.iter().enumerate() {
        eprintln!(
            "dsp{w}: {} jobs, {:.1} % busy, {} blocks lost",
            st.jobs,
            100.0 * st.busy.as_secs_f64() / secs,
            st.rx_dropped
        );
        assert_eq!(st.rx_dropped, 0);
    }
    for (k, s) in sessions[..12].iter().enumerate() {
        assert!(
            s.audio.len() as f64 > 0.9 * 12_000.0 * secs - 2400.0,
            "session {k}: {} samples in {secs:.2} s",
            s.audio.len()
        );
        assert_eq!(s.gaps, 0, "session {k}");
        assert_eq!(s.drops.load(Ordering::Relaxed), 0, "session {k}");
        let f = dominant_hz(&s.audio, 12_000.0);
        assert!((f - pitch(k)).abs() < 10.0, "session {k}: tone at {f} Hz");
    }
    assert!(sessions[12].audio.is_empty(), "the 13th has no channel");

    // session 0 leaves: the 13th gets its channel, tuned to its dial
    let gone = sessions.remove(0);
    drop(gone);
    let last = sessions.last_mut().unwrap();
    assert_eq!(last.lease.state(), LeaseState::Granted { ch: 0 });
    last.cmd
        .send(AudioCmd::Channel(last.lease.channel()))
        .unwrap();
    let t1 = Instant::now();
    while last.audio.len() < 24_000 && t1.elapsed() < Duration::from_secs(5) {
        tokio::time::sleep(Duration::from_millis(20)).await;
        last.drain();
    }
    let f = dominant_hz(&last.audio, 12_000.0);
    assert!((f - pitch(12)).abs() < 10.0, "13th: tone at {f} Hz");
    // the leaver's job ended
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(pool.stats().iter().map(|s| s.jobs).sum::<usize>(), 12);

    stop.store(true, Ordering::Relaxed);
    hw.join().unwrap().unwrap();
}
