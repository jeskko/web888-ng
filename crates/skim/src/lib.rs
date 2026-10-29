// SPDX-License-Identifier: GPL-3.0-or-later
//! Skimmers: background decoders on Skimmer-class leases
//! (`doc/m4-skimmers.md` §4).
//!
//! Each configured band holds a lease and runs a [`BandJob`] on the DSP
//! pool, which turns its channel into USB audio and cuts it into UTC
//! slots. Slots go to decoder threads at idle priority, which share the
//! time until the next slot between the bands waiting, and publish what
//! they decode on the [`SpotBus`]. The slots of one boundary are gathered
//! first; the earliest deadline goes first, and of those due together the
//! band that cost least last time, so the quiet bands are done quickly
//! and the busy ones have the time left. [`Skimmers`] starts and stops bands as
//! the config and the hours of the day say.

pub mod events;
pub mod gpsd;
pub mod mqtt;
pub mod pskreporter;
pub mod slots;
pub mod spot;
pub mod syslog;
pub mod usb;
pub mod utc;
pub mod wsprnet;

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

/// A mode's slot length, how much of it to collect before decoding, and
/// when decoding of a slot must be done (after its start: before the
/// next slot is handed out at its collect time, with a margin).
struct Timing {
    period: f64,
    collect: f64,
    done_by: f64,
}

/// FT8 transmissions end 13.1 s in at the nominal start; FT4's 5.5 s.
/// Both are searched up to about a second late.
fn timing(mode: SkimMode) -> Timing {
    match mode {
        SkimMode::Ft8 => Timing {
            period: 15.0,
            collect: 14.0,
            done_by: 15.0 + 13.5,
        },
        SkimMode::Ft4 => Timing {
            period: 7.5,
            collect: 6.6,
            done_by: 7.5 + 6.3,
        },
        // WSPR starts 1 s after the even minute and lasts 110.6 s; wsprd
        // reads 114 s.
        SkimMode::Wspr => Timing {
            period: 120.0,
            collect: 114.0,
            done_by: 120.0 + 112.0,
        },
    }
}

pub(crate) fn unix_now() -> f64 {
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
    /// Decoder CPU time, microseconds: in all, and of the last slot.
    pub cpu_us: AtomicU64,
    pub last_cpu_us: AtomicU64,
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
    queued: Instant,
}

impl Work {
    fn deadline(&self) -> f64 {
        self.start + timing(self.band.cfg.mode).done_by
    }
}

/// Slots waiting for a decoder thread.
struct Queue {
    work: Mutex<VecDeque<Work>>,
    ready: Condvar,
    /// Slots allowed to wait.
    cap: usize,
}

/// How long the first slot of a boundary waits for the other bands'.
const GATHER: Duration = Duration::from_millis(250);

/// The slot to decode next, of (deadline, last CPU) keys: the earliest
/// deadline, then the cheapest band.
fn pick(keys: impl Iterator<Item = (f64, u64)>) -> Option<usize> {
    keys.enumerate()
        .min_by(|(_, a), (_, b)| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
        .map(|(k, _)| k)
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
            if q.len() >= self.queue.cap {
                self.band.status.dropped.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            q.push_back(Work {
                band: self.band.clone(),
                start: s.start,
                audio: s.audio,
                live: self.live,
                queued: Instant::now(),
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
        // The system call itself: musl's sched_setscheduler() is a stub
        // that always fails with ENOSYS (seen on the board's kernel in the
        // emulator). Pid 0 is the calling thread.
        // SAFETY: plain syscall on the calling thread, `p` outlives it.
        let r = unsafe {
            libc::syscall(
                libc::SYS_sched_setscheduler,
                0,
                libc::SCHED_IDLE,
                &p as *const libc::sched_param,
            )
        };
        if r != 0 {
            events::emit(
                events::Level::Warning,
                "skim",
                format!("SCHED_IDLE: {}", std::io::Error::last_os_error()),
            );
        }
    }
}

fn decoder_thread(queue: Arc<Queue>, bus: Arc<SpotBus>, threads: usize) {
    idle_priority();
    let mut ft8 = web888_ftx::ft8::Decoder::new(Default::default());
    let mut ft4 = web888_ftx::ft4::Decoder::new(Default::default());
    let mut wspr = web888_wspr::Decoder::new(Default::default());
    let mut hashes = web888_ftx::msg::CallHashes::default();
    loop {
        let (w, waiting) = {
            let mut q = queue.work.lock().unwrap();
            loop {
                let Some(first) = q.iter().map(|w| w.queued).min() else {
                    q = queue.ready.wait(q).unwrap();
                    continue;
                };
                let wait = GATHER.saturating_sub(first.elapsed());
                if !wait.is_zero() {
                    q = queue.ready.wait_timeout(q, wait).unwrap().0;
                    continue;
                }
                let keys = q.iter().map(|w| {
                    let cost = w.band.status.last_cpu_us.load(Ordering::Relaxed);
                    (w.deadline(), cost)
                });
                let k = pick(keys).expect("not empty");
                let w = q.remove(k).expect("in range");
                break (w, q.len());
            }
        };
        let st = &w.band.status;
        let now = unix_now();
        // Share the time left between this slot and those still waiting
        // for this thread.
        let left = (w.start + timing(w.band.cfg.mode).done_by - now).max(0.0);
        let share = left / (1.0 + waiting as f64 / threads as f64);
        let deadline = Instant::now() + Duration::from_secs_f64(share);
        let cpu0 = thread_cpu();
        let cut0 = ft8.stats.cut_short + ft4.stats.cut_short + wspr.cut_short;
        let dial = w.band.cfg.dial;
        let spot = |mode, freq: f64, snr, dt, message: String, sender, grid| Spot {
            time: w.start,
            mode,
            dial_hz: dial,
            freq_hz: dial + freq,
            snr_db: snr,
            dt_s: dt,
            message,
            sender,
            grid,
            drift: None,
            dbm: None,
            live: w.live,
        };
        let ftx = |mode, ds: Vec<web888_ftx::ft8::Decode>| -> Vec<Spot> {
            ds.into_iter()
                .map(|d| {
                    let m = d.message;
                    spot(mode, d.freq as f64, d.snr, d.dt, m.text, m.sender, m.grid)
                })
                .collect()
        };
        let ds: Vec<Spot> = match w.band.cfg.mode {
            SkimMode::Ft8 => ftx(
                "FT8",
                ft8.decode_until(&w.audio, &mut hashes, Some(deadline)),
            ),
            SkimMode::Ft4 => ftx(
                "FT4",
                ft4.decode_until(&w.audio, &mut hashes, Some(deadline)),
            ),
            SkimMode::Wspr => wspr
                .decode_until(&w.audio, Some(deadline))
                .into_iter()
                .map(|d| {
                    let m = d.message;
                    Spot {
                        drift: Some(d.drift),
                        dbm: Some(m.dbm),
                        ..spot("WSPR", d.freq, d.snr, d.dt, m.text, m.call, m.grid)
                    }
                })
                .collect(),
        };
        let cpu = (thread_cpu() - cpu0).as_micros() as u64;
        st.cpu_us.fetch_add(cpu, Ordering::Relaxed);
        st.last_cpu_us.store(cpu, Ordering::Relaxed);
        st.slots.fetch_add(1, Ordering::Relaxed);
        st.cut_short.fetch_add(
            ft8.stats.cut_short + ft4.stats.cut_short + wspr.cut_short - cut0,
            Ordering::Relaxed,
        );
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
            bus.publish(d);
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
        let queue = Arc::new(Queue {
            work: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
            cap: MAX_QUEUED * threads.max(1),
        });
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
                events::emit(
                    events::Level::Notice,
                    "skim",
                    format!("stop {}", r.band.label),
                );
                r.stop.store(true, Ordering::Relaxed);
            }
            keep
        });
        for w in want {
            if running.iter().any(|r| r.band.cfg == *w) {
                continue;
            }
            let label = format!("{:?} {:.1} kHz", w.mode, w.dial / 1e3).to_lowercase();
            events::emit(events::Level::Notice, "skim", format!("start {label}"));
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
                slotter: {
                    let t = timing(w.mode);
                    Slotter::new(t.period, t.collect, AUDIO_RATE)
                },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn earliest_deadline_then_cheapest_band() {
        // FT8 slots due at 28.5 s, an FT4 one due at 13.8 s
        let keys = [(28.5, 5_000_000), (28.5, 500_000), (13.8, 900_000)];
        assert_eq!(pick(keys.into_iter()), Some(2));
        assert_eq!(pick(keys[..2].iter().copied()), Some(1));
        assert_eq!(pick(std::iter::empty()), None);
    }
}
