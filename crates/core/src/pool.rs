// SPDX-License-Identifier: GPL-3.0-or-later
//! The DSP pool (`doc/m3-multi-user.md` §4): a few worker threads (one per
//! core, pinned) run every session's per-block DSP, instead of a thread
//! per session.
//!
//! Each worker subscribes once to the hw thread's RX and WF blocks and
//! runs the [`Job`]s placed on it in its own thread, so a job's state
//! never moves between threads while it runs: every RX block and WF block
//! goes to every job, and after each round (an RX block, ~5 ms, or
//! [`TICK`] without one) every job gets a tick for timed output. A new
//! job goes to the worker with the fewest jobs. A job ends when it says so
//! (its session is gone); jobs aren't migrated when others leave, so new
//! ones fill the lighter worker first.
//!
//! Load shedding: a worker busy more than [`SHED_ABOVE`] of a second
//! raises its shed level (jobs see it in [`Ctx`]; waterfalls halve their
//! FFT budget per level, then their rate), and lowers it again below
//! [`SHED_BELOW`]. Audio is never shed. Busy is wall time, preemption
//! included, on purpose: it is the contention that makes audio late. (By
//! the workers' CPU time the views kept their rate with 16 clients on the
//! unit, the box saturated and every client's audio broke up.)

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};
use web888_hw::{HwCmd, Subscriber};
use web888_radio::{RxBlock, WfBlock};

/// What a worker tells its jobs each tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ctx {
    /// Load shedding level: 0 = none.
    pub shed: u32,
}

/// Work for one client (a session's audio chain, a waterfall view). Each
/// method returns false when the job is finished.
pub trait Job: Send + 'static {
    fn rx(&mut self, _b: &RxBlock) -> bool {
        true
    }
    fn wf(&mut self, _b: &Arc<WfBlock>) -> bool {
        true
    }
    /// After each round: timed output, checking the client is still there.
    fn tick(&mut self, _now: Instant, _ctx: Ctx) -> bool {
        true
    }
}

/// RX blocks queued per worker: 64 blocks of ~5 ms.
const DEPTH: usize = 64;
/// WF blocks queued per worker.
const WF_DEPTH: usize = 256;
/// Longest time between ticks.
pub const TICK: Duration = Duration::from_millis(10);
/// Busy share of a second above which a worker sheds load, and below
/// which it stops.
pub const SHED_ABOVE: f64 = 0.7;
pub const SHED_BELOW: f64 = 0.4;
const MAX_SHED: u32 = 8;

#[derive(Default)]
struct Load {
    jobs: AtomicUsize,
    /// Time spent in jobs, ns.
    busy_ns: AtomicU64,
    shed: std::sync::atomic::AtomicU32,
}

struct Worker {
    jobs: Sender<Box<dyn Job>>,
    load: Arc<Load>,
    rx_dropped: Arc<AtomicU64>,
    wf_dropped: Arc<AtomicU64>,
}

/// Where a job runs.
pub struct Placement {
    pub worker: usize,
    /// RX blocks the worker lost (its queue was full); shared by its jobs.
    pub rx_dropped: Arc<AtomicU64>,
    /// WF blocks the worker lost.
    pub wf_dropped: Arc<AtomicU64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorkerStats {
    pub jobs: usize,
    pub busy: Duration,
    pub rx_dropped: u64,
    pub wf_dropped: u64,
    pub shed: u32,
}

pub struct DspPool {
    workers: Vec<Worker>,
}

impl DspPool {
    /// `n` workers (at least one), subscribed to the hw thread's RX and
    /// WF blocks; worker k pinned to CPU k if `pin`.
    pub fn new(n: usize, hw: &Sender<HwCmd>, pin: bool) -> std::io::Result<Self> {
        let mut workers = Vec::new();
        for k in 0..n.max(1) {
            let (sub, blocks, rx_dropped) = Subscriber::new(DEPTH);
            let (wf_sub, wf_blocks, wf_dropped) = Subscriber::new(WF_DEPTH);
            let (jobs, jobs_rx) = channel();
            let load = Arc::new(Load::default());
            let l = load.clone();
            std::thread::Builder::new()
                .name(format!("dsp{k}"))
                .spawn(move || {
                    if pin {
                        pin_to_cpu(k);
                    }
                    run(blocks, wf_blocks, jobs_rx, l)
                })?;
            let _ = hw.send(HwCmd::SubscribeRx(sub));
            let _ = hw.send(HwCmd::SubscribeWf(wf_sub));
            workers.push(Worker {
                jobs,
                load,
                rx_dropped,
                wf_dropped,
            });
        }
        Ok(DspPool { workers })
    }

    /// One worker per available CPU.
    pub fn per_cpu(hw: &Sender<HwCmd>) -> std::io::Result<Self> {
        let n = std::thread::available_parallelism().map_or(1, |n| n.get());
        Self::new(n, hw, true)
    }

    pub fn workers(&self) -> usize {
        self.workers.len()
    }

    /// Run `job` on the least loaded worker.
    pub fn add(&self, job: Box<dyn Job>) -> Placement {
        let (k, w) = self
            .workers
            .iter()
            .enumerate()
            .min_by_key(|(_, w)| w.load.jobs.load(Ordering::Relaxed))
            .expect("at least one worker");
        w.load.jobs.fetch_add(1, Ordering::Relaxed);
        if w.jobs.send(job).is_err() {
            w.load.jobs.fetch_sub(1, Ordering::Relaxed);
        }
        Placement {
            worker: k,
            rx_dropped: w.rx_dropped.clone(),
            wf_dropped: w.wf_dropped.clone(),
        }
    }

    pub fn stats(&self) -> Vec<WorkerStats> {
        self.workers
            .iter()
            .map(|w| WorkerStats {
                jobs: w.load.jobs.load(Ordering::Relaxed),
                busy: Duration::from_nanos(w.load.busy_ns.load(Ordering::Relaxed)),
                rx_dropped: w.rx_dropped.load(Ordering::Relaxed),
                wf_dropped: w.wf_dropped.load(Ordering::Relaxed),
                shed: w.load.shed.load(Ordering::Relaxed),
            })
            .collect()
    }
}

/// The calling thread's CPU time.
pub fn thread_cpu() -> Duration {
    let mut t = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: plain syscall writing into `t`.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
    Duration::new(t.tv_sec as u64, t.tv_nsec as u32)
}

fn run(
    blocks: Receiver<Arc<RxBlock>>,
    wf_blocks: Receiver<Arc<WfBlock>>,
    new: Receiver<Box<dyn Job>>,
    load: Arc<Load>,
) {
    let mut jobs: Vec<Box<dyn Job>> = Vec::new();
    let mut ctx = Ctx::default();
    let mut window = Instant::now();
    let mut busy = Duration::ZERO;
    loop {
        loop {
            match new.try_recv() {
                Ok(j) => jobs.push(j),
                Err(TryRecvError::Empty) => break,
                // the pool is gone: finish
                Err(TryRecvError::Disconnected) => return,
            }
        }
        let before = jobs.len();
        let rx = blocks.recv_timeout(TICK);
        let t = Instant::now();
        match rx {
            Ok(b) => jobs.retain_mut(|j| j.rx(&b)),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        while let Ok(b) = wf_blocks.try_recv() {
            jobs.retain_mut(|j| j.wf(&b));
        }
        let now = Instant::now();
        jobs.retain_mut(|j| j.tick(now, ctx));
        let spent = t.elapsed();
        busy += spent;
        load.busy_ns
            .fetch_add(spent.as_nanos() as u64, Ordering::Relaxed);
        if jobs.len() != before {
            load.jobs.fetch_sub(before - jobs.len(), Ordering::Relaxed);
        }
        // shedding, judged once a second
        let span = now.duration_since(window);
        if span >= Duration::from_secs(1) {
            let f = busy.as_secs_f64() / span.as_secs_f64();
            if f > SHED_ABOVE && ctx.shed < MAX_SHED {
                ctx.shed += 1;
            } else if f < SHED_BELOW && ctx.shed > 0 {
                ctx.shed -= 1;
            }
            load.shed.store(ctx.shed, Ordering::Relaxed);
            window = now;
            busy = Duration::ZERO;
        }
    }
}

#[cfg(target_os = "linux")]
fn pin_to_cpu(k: usize) {
    // SAFETY: a zeroed cpu_set_t is a valid empty set; the call only
    // reads it
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_SET(k, &mut set);
        if libc::sched_setaffinity(0, std::mem::size_of_val(&set), &set) != 0 {
            eprintln!("dsp{k}: not pinned: {}", std::io::Error::last_os_error());
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn pin_to_cpu(_: usize) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records the blocks it saw; ends after `limit` blocks.
    struct Count {
        seen: Arc<Mutex<Vec<u64>>>,
        limit: usize,
    }

    impl Job for Count {
        fn rx(&mut self, b: &RxBlock) -> bool {
            let mut s = self.seen.lock().unwrap();
            s.push(b.first_frame);
            s.len() < self.limit
        }
    }

    fn block(first_frame: u64) -> RxBlock {
        RxBlock {
            first_frame,
            nch: 1,
            iq: vec![Default::default(); 10],
            overload: false,
        }
    }

    /// A fake hw thread: the pool's subscriptions, fed by hand.
    fn pool(n: usize) -> (DspPool, web888_hw::Fanout<RxBlock>, usize) {
        let (tx, rx) = channel();
        let p = DspPool::new(n, &tx, false).unwrap();
        let mut f = web888_hw::Fanout::default();
        let mut subs = 0;
        for c in rx.try_iter() {
            match c {
                HwCmd::SubscribeRx(s) => {
                    f.add(s);
                    subs += 1;
                }
                HwCmd::SubscribeWf(_) => {}
                _ => panic!("subscription expected"),
            }
        }
        (p, f, subs)
    }

    fn wait_for(cond: impl Fn() -> bool) {
        let t = Instant::now();
        while !cond() {
            assert!(t.elapsed() < Duration::from_secs(2), "timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn jobs_spread_over_workers_and_end_themselves() {
        let (p, mut hw, subs) = pool(2);
        assert_eq!(subs, 2);
        let seen: Vec<_> = (0..3).map(|_| Arc::new(Mutex::new(Vec::new()))).collect();
        let places: Vec<usize> = seen
            .iter()
            .enumerate()
            .map(|(k, s)| {
                p.add(Box::new(Count {
                    seen: s.clone(),
                    limit: 2 + k,
                }))
                .worker
            })
            .collect();
        assert_eq!(places, [0, 1, 0], "least loaded first");
        wait_for(|| p.stats().iter().map(|s| s.jobs).sum::<usize>() == 3);
        // give the workers time to pick the jobs up before the blocks
        std::thread::sleep(Duration::from_millis(150));
        for f in 0..5 {
            hw.send(block(f * 10));
        }
        wait_for(|| p.stats().iter().all(|s| s.jobs == 0));
        let got: Vec<Vec<u64>> = seen.iter().map(|s| s.lock().unwrap().clone()).collect();
        assert_eq!(got, [vec![0, 10], vec![0, 10, 20], vec![0, 10, 20, 30]]);
        // the next job goes to a now empty worker
        let s = Arc::new(Mutex::new(Vec::new()));
        assert_eq!(p.add(Box::new(Count { seen: s, limit: 1 })).worker, 0);
    }

    /// Keeps its worker ~90 % busy; records the shed level it's told.
    struct Hog(Arc<std::sync::atomic::AtomicU32>);

    impl Job for Hog {
        fn tick(&mut self, _: Instant, ctx: Ctx) -> bool {
            self.0.store(ctx.shed, Ordering::Relaxed);
            // CPU, not sleep: shedding goes by the worker's CPU time
            let t = Instant::now();
            while t.elapsed() < TICK * 9 {
                std::hint::black_box(0u64);
            }
            true
        }
    }

    #[test]
    fn a_busy_worker_sheds_load() {
        let (p, _hw, _) = pool(1);
        let level = Arc::new(std::sync::atomic::AtomicU32::new(0));
        p.add(Box::new(Hog(level.clone())));
        let t = Instant::now();
        while level.load(Ordering::Relaxed) == 0 {
            assert!(t.elapsed() < Duration::from_secs(4), "no shedding");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(p.stats()[0].shed >= 1);
    }
}
