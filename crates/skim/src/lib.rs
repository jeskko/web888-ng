// SPDX-License-Identifier: GPL-3.0-or-later
//! Skimmers: background decoders on Skimmer-class leases
//! (`doc/m4-skimmers.md` §4).
//!
//! Each configured band holds a lease and runs a [`BandJob`] on the DSP
//! pool, which turns its channel into USB audio and cuts it into UTC
//! slots. Slots go to decoder threads at idle priority, which share the
//! time until the next slot between the bands waiting, and publish what
//! they decode on the [`SpotBus`]. [`Skimmers`] starts and stops bands as
//! the config and the hours of the day say.

pub mod pskreporter;
pub mod slots;
pub mod spot;
pub mod usb;

pub use spot::{Spot, SpotBus};

use slots::{FrameClock, Slotter};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};
use usb::{AUDIO_RATE, OFFSET_HZ, UsbAudio};
use web888_config::{SkimMode, Skimmer};
use web888_core::leases::{Channels, Class, Lease};
use web888_core::pool::{DspPool, Job};
use web888_radio::RxBlock;

/// FT8 slot length and how much of it to collect before decoding
/// (transmissions end 13.1 s in at the nominal start).
const FT8_PERIOD: f64 = 15.0;
const FT8_COLLECT: f64 = 14.0;
/// Decoding of a slot must be done this long after the slot starts (the
/// next slot is handed out at its 14.0 s).
const FT8_DONE_BY: f64 = FT8_PERIOD + 13.5;

fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// What a band reports (admin page, metrics).
#[derive(Default)]
pub struct BandStatus {
    /// Slots decoded, spots, slots the deadline cut short.
    pub slots: AtomicU64,
    pub spots: AtomicU64,
    pub cut_short: AtomicU64,
    /// Slots dropped because the decoders were behind.
    pub dropped: AtomicU64,
    /// Decoder CPU time, microseconds.
    pub cpu_us: AtomicU64,
    /// Slot start (UTC seconds) of the last slot decoded.
    pub last_slot: AtomicU64,
    /// Channel granted (0 = waiting).
    pub granted: AtomicBool,
}

/// A band: its config and status.
pub struct Band {
    pub cfg: Skimmer,
    pub label: String,
    pub status: BandStatus,
}

/// A slot of audio to decode.
struct Work {
    band: Arc<Band>,
    start: f64,
    audio: Vec<f32>,
    live: bool,
}

/// Slots waiting for a decoder thread.
#[derive(Default)]
struct Queue {
    work: Mutex<VecDeque<Work>>,
    ready: Condvar,
}

/// The pool job of one band.
pub struct BandJob {
    band: Arc<Band>,
    lease: Lease,
    ch: Option<usize>,
    usb: UsbAudio,
    clock: FrameClock,
    slotter: Slotter,
    rx_rate: f64,
    next_frame: Option<u64>,
    audio: Vec<f32>,
    /// Frame of audio sample 0, and samples so far.
    audio_base: Option<f64>,
    audio_count: u64,
    queue: Arc<Queue>,
    live: bool,
    stop: Arc<AtomicBool>,
}

/// Slots allowed to wait per decoder thread before new ones are dropped.
const MAX_QUEUED: usize = 8;

impl Job for BandJob {
    fn rx(&mut self, b: &RxBlock) -> bool {
        if self.stop.load(Ordering::Relaxed) {
            return false;
        }
        let ch = self.lease.channel();
        self.band
            .status
            .granted
            .store(ch.is_some(), Ordering::Relaxed);
        if ch != self.ch || self.next_frame != Some(b.first_frame) {
            // another channel, or a gap: start the audio over
            self.ch = ch;
            self.usb = UsbAudio::new(self.rx_rate);
            self.slotter.reset();
            self.audio_base = None;
            self.audio_count = 0;
        }
        self.next_frame = Some(b.end_frame());
        let Some(ch) = ch else { return true };
        self.clock.observe(b.end_frame(), unix_now());
        self.audio.clear();
        self.usb
            .process(b.channel(ch).map(web888_dsp::rx_to_c32), &mut self.audio);
        // Audio sample i (counted since the audio restarted) is frame
        // base + i * decimation; the filter's delay is a millisecond, well
        // inside FT8's timing tolerance.
        let r = self.rx_rate / AUDIO_RATE;
        let base = *self.audio_base.get_or_insert(b.first_frame as f64);
        let first = self.audio_count;
        self.audio_count += self.audio.len() as u64;
        let clock = &self.clock;
        let slots = self.slotter.push(
            first,
            &self.audio,
            |i| clock.utc(base + i * r),
            |t| clock.frame(t).map(|f| (f - base) / r),
        );
        for s in slots {
            let mut q = self.queue.work.lock().unwrap();
            if q.len() >= MAX_QUEUED {
                self.band.status.dropped.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            q.push_back(Work {
                band: self.band.clone(),
                start: s.start,
                audio: s.audio,
                live: self.live,
            });
            self.queue.ready.notify_one();
        }
        true
    }

    fn tick(&mut self, _now: Instant, _ctx: web888_core::pool::Ctx) -> bool {
        !self.stop.load(Ordering::Relaxed)
    }
}

fn thread_cpu() -> Duration {
    let mut t = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: plain syscall writing into `t`.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
    Duration::new(t.tv_sec as u64, t.tv_nsec as u32)
}

/// Run this thread only when nothing else wants the CPU.
fn idle_priority() {
    #[cfg(target_os = "linux")]
    {
        // zeroed: 32-bit musl's sched_param has more fields (SCHED_SPORADIC)
        // SAFETY: an all-zero sched_param is valid.
        let p: libc::sched_param = unsafe { std::mem::zeroed() };
        // SAFETY: plain syscall on the calling thread.
        if unsafe { libc::sched_setscheduler(0, libc::SCHED_IDLE, &p) } != 0 {
            eprintln!("skim: SCHED_IDLE: {}", std::io::Error::last_os_error());
        }
    }
}

fn decoder_thread(queue: Arc<Queue>, bus: Arc<SpotBus>, threads: usize) {
    idle_priority();
    let mut ft8 = web888_ftx::ft8::Decoder::new(Default::default());
    let mut hashes = web888_ftx::msg::CallHashes::default();
    loop {
        let (w, waiting) = {
            let mut q = queue.work.lock().unwrap();
            loop {
                if let Some(w) = q.pop_front() {
                    break (w, q.len());
                }
                q = queue.ready.wait(q).unwrap();
            }
        };
        let st = &w.band.status;
        let now = unix_now();
        // Share the time left between this slot and those still waiting
        // for this thread.
        let left = (w.start + FT8_DONE_BY - now).max(0.0);
        let share = left / (1.0 + waiting as f64 / threads as f64);
        let deadline = Instant::now() + Duration::from_secs_f64(share);
        let cpu0 = thread_cpu();
        let cut0 = ft8.stats.cut_short;
        let ds = match w.band.cfg.mode {
            SkimMode::Ft8 => ft8.decode_until(&w.audio, &mut hashes, Some(deadline)),
        };
        st.cpu_us
            .fetch_add((thread_cpu() - cpu0).as_micros() as u64, Ordering::Relaxed);
        st.slots.fetch_add(1, Ordering::Relaxed);
        st.cut_short
            .fetch_add(ft8.stats.cut_short - cut0, Ordering::Relaxed);
        st.last_slot.store(w.start as u64, Ordering::Relaxed);
        st.spots.fetch_add(ds.len() as u64, Ordering::Relaxed);
        if std::env::var_os("WEB888_SKIM_DEBUG").is_some() {
            eprintln!(
                "skim: {} slot {}: {} decodes, {:.0} ms CPU, {} queued",
                w.band.label,
                w.start as u64 % 86_400,
                ds.len(),
                (thread_cpu() - cpu0).as_secs_f64() * 1e3,
                waiting
            );
        }
        for d in ds {
            bus.publish(Spot {
                time: w.start,
                mode: "FT8",
                dial_hz: w.band.cfg.dial,
                freq_hz: w.band.cfg.dial + d.freq as f64,
                snr_db: d.snr,
                dt_s: d.dt,
                sender: d.message.sender.clone(),
                grid: d.message.grid.clone(),
                message: d.message.text,
                live: w.live,
            });
        }
    }
}

struct Running {
    band: Arc<Band>,
    stop: Arc<AtomicBool>,
}

/// The skimmer service: bands from the config, decoder threads, spots.
pub struct Skimmers {
    channels: Arc<Channels>,
    pool: Arc<DspPool>,
    rx_rate: f64,
    live: bool,
    queue: Arc<Queue>,
    pub bus: Arc<SpotBus>,
    running: Mutex<Vec<Running>>,
}

impl Skimmers {
    /// `live`: the radio receives live signals (see `Identity::live`).
    pub fn new(
        channels: Arc<Channels>,
        pool: Arc<DspPool>,
        rx_rate: f64,
        live: bool,
        threads: usize,
    ) -> std::io::Result<Arc<Self>> {
        let queue = Arc::new(Queue::default());
        let bus = SpotBus::new(10_000);
        for k in 0..threads.max(1) {
            let (q, b) = (queue.clone(), bus.clone());
            std::thread::Builder::new()
                .name(format!("skim{k}"))
                .spawn(move || decoder_thread(q, b, threads.max(1)))?;
        }
        Ok(Arc::new(Skimmers {
            channels,
            pool,
            rx_rate,
            live,
            queue,
            bus,
            running: Mutex::new(vec![]),
        }))
    }

    /// Run the bands of `cfg` that are enabled at UTC hour `hour`; stop
    /// the others.
    pub fn apply(&self, cfg: &[Skimmer], hour: u32) {
        let want: Vec<&Skimmer> = cfg
            .iter()
            .filter(|s| s.enabled && s.dial > 0.0 && s.runs_at(hour))
            .collect();
        let mut running = self.running.lock().unwrap();
        running.retain(|r| {
            let keep = want.iter().any(|w| **w == r.band.cfg);
            if !keep {
                eprintln!("skim: stop {}", r.band.label);
                r.stop.store(true, Ordering::Relaxed);
            }
            keep
        });
        for w in want {
            if running.iter().any(|r| r.band.cfg == *w) {
                continue;
            }
            let label = format!("{:?} {:.1} kHz", w.mode, w.dial / 1e3).to_lowercase();
            eprintln!("skim: start {label}");
            let band = Arc::new(Band {
                cfg: w.clone(),
                label: label.clone(),
                status: BandStatus::default(),
            });
            let stop = Arc::new(AtomicBool::new(false));
            let lease = self
                .channels
                .acquire(Class::Skimmer, &label, w.dial + OFFSET_HZ);
            self.pool.add(Box::new(BandJob {
                band: band.clone(),
                lease,
                ch: None,
                usb: UsbAudio::new(self.rx_rate),
                clock: FrameClock::new(self.rx_rate),
                slotter: Slotter::new(FT8_PERIOD, FT8_COLLECT, AUDIO_RATE),
                rx_rate: self.rx_rate,
                next_frame: None,
                audio: vec![],
                audio_base: None,
                audio_count: 0,
                queue: self.queue.clone(),
                live: self.live,
                stop: stop.clone(),
            }));
            running.push(Running { band, stop });
        }
    }

    /// The bands running now.
    pub fn bands(&self) -> Vec<Arc<Band>> {
        self.running
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.band.clone())
            .collect()
    }
}

impl Drop for Skimmers {
    fn drop(&mut self) {
        for r in self.running.lock().unwrap().iter() {
            r.stop.store(true, Ordering::Relaxed);
        }
    }
}
