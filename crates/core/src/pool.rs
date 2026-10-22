// SPDX-License-Identifier: GPL-3.0-or-later
//! The DSP pool (`doc/m3-multi-user.md` §4): a few worker threads (one per
//! core, pinned) run every session's per-block DSP, instead of a thread
//! per session.
//!
//! Each worker subscribes once to the hw thread's RX blocks and runs the
//! [`RxJob`]s placed on it, block by block, in its own thread, so a job's
//! state never moves between threads while it runs. A new job goes to the
//! worker with the fewest jobs. A job ends when it says so (its session is
//! gone); jobs aren't migrated when others leave, so new ones fill the
//! lighter worker first.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};
use web888_hw::{HwCmd, Subscriber};
use web888_radio::RxBlock;

/// Per-block work for one client (e.g. a session's audio chain).
pub trait RxJob: Send + 'static {
    /// Process one RX block; false when the job is finished.
    fn process(&mut self, b: &RxBlock) -> bool;
    /// No block for a while (radio stopped); false when finished.
    fn idle(&mut self) -> bool {
        true
    }
}

/// RX blocks queued per worker: 64 blocks of ~5 ms.
const DEPTH: usize = 64;
const IDLE: Duration = Duration::from_millis(100);

#[derive(Default)]
struct Load {
    jobs: AtomicUsize,
    /// Time spent in jobs, ns.
    busy_ns: AtomicU64,
}

struct Worker {
    jobs: Sender<Box<dyn RxJob>>,
    load: Arc<Load>,
    rx_dropped: Arc<AtomicU64>,
}

/// Where a job runs.
pub struct Placement {
    pub worker: usize,
    /// RX blocks the worker lost (its queue was full); shared by its jobs.
    pub rx_dropped: Arc<AtomicU64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorkerStats {
    pub jobs: usize,
    pub busy: Duration,
    pub rx_dropped: u64,
}

pub struct DspPool {
    workers: Vec<Worker>,
}

impl DspPool {
    /// `n` workers (at least one), subscribed to the hw thread's RX
    /// blocks; worker k pinned to CPU k if `pin`.
    pub fn new(n: usize, hw: &Sender<HwCmd>, pin: bool) -> std::io::Result<Self> {
        let mut workers = Vec::new();
        for k in 0..n.max(1) {
            let (sub, blocks, rx_dropped) = Subscriber::new(DEPTH);
            let (jobs, jobs_rx) = channel();
            let load = Arc::new(Load::default());
            let l = load.clone();
            std::thread::Builder::new()
                .name(format!("dsp{k}"))
                .spawn(move || {
                    if pin {
                        pin_to_cpu(k);
                    }
                    run(blocks, jobs_rx, l)
                })?;
            let _ = hw.send(HwCmd::SubscribeRx(sub));
            workers.push(Worker {
                jobs,
                load,
                rx_dropped,
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
    pub fn add(&self, job: Box<dyn RxJob>) -> Placement {
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
        }
    }

    pub fn stats(&self) -> Vec<WorkerStats> {
        self.workers
            .iter()
            .map(|w| WorkerStats {
                jobs: w.load.jobs.load(Ordering::Relaxed),
                busy: Duration::from_nanos(w.load.busy_ns.load(Ordering::Relaxed)),
                rx_dropped: w.rx_dropped.load(Ordering::Relaxed),
            })
            .collect()
    }
}

fn run(blocks: Receiver<Arc<RxBlock>>, new: Receiver<Box<dyn RxJob>>, load: Arc<Load>) {
    let mut jobs: Vec<Box<dyn RxJob>> = Vec::new();
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
        match blocks.recv_timeout(IDLE) {
            Ok(b) => {
                let t = Instant::now();
                jobs.retain_mut(|j| j.process(&b));
                load.busy_ns
                    .fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
            }
            Err(RecvTimeoutError::Timeout) => jobs.retain_mut(|j| j.idle()),
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if jobs.len() != before {
            load.jobs.fetch_sub(before - jobs.len(), Ordering::Relaxed);
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

    impl RxJob for Count {
        fn process(&mut self, b: &RxBlock) -> bool {
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
                HwCmd::SubscribeRx(s) => f.add(s),
                _ => panic!("subscription expected"),
            }
            subs += 1;
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
}
