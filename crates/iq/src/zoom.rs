// SPDX-License-Identifier: GPL-3.0-or-later
//! The iq backend's zoom engine: the bitstream's (`hw/docs/channelizer.md`
//! §3.9) limits and timing, its arithmetic in floats. Jobs wait in a
//! queue of [`ZoomCaps::queue`], in order; a job starts at a frame when
//! the contexts it needs are free (one for up to half the work RAM's
//! samples, both for more), captures its bins for n frames, and its
//! result is ready at the end of the last (the FFTs take no time here).
//! Samples saturate as the work RAM's 22 bits do after the job's shift.

use std::collections::VecDeque;
use std::sync::Arc;
use web888_dsp::C32;
use web888_radio::bins::{ZoomCaps, ZoomJob, ZoomResult};

/// Results kept for a reader that falls behind.
const KEEP: usize = 64;

struct Running {
    job: ZoomJob,
    /// Contexts held (bit per context).
    ctx: u8,
    first: u64,
    got: usize,
    /// Bin b's frame t at `b * n + t`.
    x: Vec<C32>,
    saturated: bool,
}

pub struct ZoomEngine {
    caps: ZoomCaps,
    /// The source's bins (a job's bin numbers wrap here).
    nbins: usize,
    queue: VecDeque<ZoomJob>,
    running: Vec<Running>,
    busy: u8,
    done: VecDeque<ZoomResult>,
    /// Results the reader lost by falling behind.
    pub lost: u64,
    windows: Vec<Option<Arc<Vec<f32>>>>,
    work: Vec<C32>,
    scratch: Vec<C32>,
}

impl ZoomEngine {
    pub fn new(caps: ZoomCaps, nbins: usize) -> Self {
        ZoomEngine {
            caps,
            nbins,
            queue: VecDeque::new(),
            running: Vec::new(),
            busy: 0,
            done: VecDeque::new(),
            lost: 0,
            windows: vec![None; caps.max_logn as usize + 1],
            work: Vec::new(),
            scratch: Vec::new(),
        }
    }

    pub fn caps(&self) -> ZoomCaps {
        self.caps
    }

    /// Queue jobs as far as there is room; returns how many. A job the
    /// engine can't take (`ZoomCaps::takes`) is taken and dropped.
    pub fn submit(&mut self, jobs: &[ZoomJob]) -> usize {
        let room = self.caps.queue - self.queue.len();
        let n = jobs.len().min(room);
        self.queue.extend(
            jobs[..n]
                .iter()
                .filter(|j| self.caps.takes(j) && (j.k0 as usize) < self.nbins),
        );
        n
    }

    /// Jobs queued or running.
    pub fn pending(&self) -> usize {
        self.queue.len() + self.running.len()
    }

    /// Frame `frame` (every bin, as the bank made it: a full-scale sine
    /// |z| = 1).
    pub fn frame(&mut self, frame: u64, u: &[C32]) {
        // jobs start at a frame, in order, as their contexts free up
        while let Some(j) = self.queue.front() {
            let need = self.caps.contexts_for(j.samples()).expect("taken");
            let free: Vec<u8> = (0..self.caps.contexts as u8)
                .filter(|c| self.busy & 1 << c == 0)
                .collect();
            if free.len() < need {
                break;
            }
            let ctx = free[..need].iter().fold(0, |m, c| m | 1 << c);
            self.busy |= ctx;
            let job = self.queue.pop_front().expect("front");
            self.running.push(Running {
                job,
                ctx,
                first: frame,
                got: 0,
                x: vec![C32::default(); job.samples()],
                saturated: false,
            });
        }
        let mut k = 0;
        while k < self.running.len() {
            let r = &mut self.running[k];
            let n = r.job.n();
            // the 25-bit bin (a full-scale sine 2^24) >> shift into 22 bits
            let lim =
                ((1u32 << 21) - 1) as f32 * (1u32 << r.job.shift) as f32 / (1u32 << 24) as f32;
            for b in 0..r.job.bins as usize {
                let z = u[(r.job.k0 as usize + b) % self.nbins];
                let c = C32::new(z.re.clamp(-lim, lim), z.im.clamp(-lim, lim));
                r.saturated |= c != z;
                r.x[b * n + r.got] = c;
            }
            r.got += 1;
            if r.got == n {
                let r = self.running.swap_remove(k);
                self.busy &= !r.ctx;
                let res = self.finish(r);
                if self.done.len() == KEEP {
                    self.done.pop_front();
                    self.lost += 1;
                }
                self.done.push_back(res);
            } else {
                k += 1;
            }
        }
    }

    fn finish(&mut self, r: Running) -> ZoomResult {
        let n = r.job.n();
        let w = self.windows[r.job.logn as usize]
            .get_or_insert_with(|| Arc::new(web888_dsp::wf::blackman_harris(n)))
            .clone();
        let fft = web888_dsp::fft_plan(n, false);
        self.work.resize(n, C32::default());
        self.scratch
            .resize(fft.get_inplace_scratch_len(), C32::default());
        let mut power = Vec::with_capacity(r.job.points());
        for x in r.x.chunks_exact(n) {
            for ((o, &s), &w) in self.work.iter_mut().zip(x).zip(w.iter()) {
                *o = s * w;
            }
            fft.process_with_scratch(&mut self.work, &mut self.scratch);
            // the middle n/2 points, ascending: -n/4 .. n/4 - 1
            power.extend((0..n / 2).map(|j| self.work[(j + n - n / 4) % n].norm_sqr()));
        }
        ZoomResult {
            tag: r.job.tag,
            frame: r.first as u16,
            saturated: r.saturated,
            power,
        }
    }

    /// Move the results since the last call to `out`; returns how many.
    pub fn read(&mut self, out: &mut Vec<ZoomResult>) -> usize {
        let n = self.done.len();
        out.extend(self.done.drain(..));
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(tag: u8, logn: u8, bins: u16) -> ZoomJob {
        ZoomJob {
            tag,
            shift: 3,
            logn,
            bins,
            k0: 10,
        }
    }

    /// Frames of 32 bins of a constant until `out` has `want` results;
    /// returns each result's tag with the frame it came after.
    fn run(e: &mut ZoomEngine, want: usize) -> Vec<(u8, u64, u16)> {
        let u = vec![C32::new(0.01, 0.0); 32];
        let mut got = Vec::new();
        let mut out = Vec::new();
        for f in 0..100_000u64 {
            e.frame(f, &u);
            out.clear();
            e.read(&mut out);
            got.extend(out.iter().map(|r| (r.tag, f, r.frame)));
            if got.len() >= want {
                break;
            }
        }
        got
    }

    #[test]
    fn jobs_share_the_contexts_in_order() {
        let mut e = ZoomEngine::new(ZoomCaps::ENGINE, 32);
        // two of 16 x 128 = 2048 (a context each), then one of 17 x 128
        // (both), then one of 2048 again
        assert_eq!(
            e.submit(&[job(1, 7, 16), job(2, 7, 16), job(3, 7, 17), job(4, 7, 8)]),
            4
        );
        let got = run(&mut e, 4);
        // 1 and 2 together (frames 0..127), 3 after both (128..255), then
        // 4 (256..383)
        assert_eq!(
            got,
            vec![(1, 127, 0), (2, 127, 0), (3, 255, 128), (4, 383, 256)]
        );
    }

    #[test]
    fn the_queue_holds_16() {
        let mut e = ZoomEngine::new(ZoomCaps::ENGINE, 32);
        let jobs: Vec<ZoomJob> = (0..20).map(|t| job(t, 3, 1)).collect();
        assert_eq!(e.submit(&jobs), 16);
        assert_eq!(e.submit(&jobs[16..]), 0);
        // a frame starts two (a context each): room for two more
        e.frame(0, &[C32::default(); 32]);
        assert_eq!(e.submit(&jobs[16..]), 2);
        assert_eq!(e.pending(), 18);
    }

    #[test]
    fn a_tone_and_saturation() {
        let mut e = ZoomEngine::new(ZoomCaps::ENGINE, 4);
        // bin 1: a full-scale tone 15 kHz / 8 = 1875 Hz (FFT bin 4 of 32)
        // above the centre; bin 2 at -6 dBFS; shift 3 saturates at full
        // scale, shift 4 doesn't
        let n = 32;
        let mut jobs = vec![ZoomJob {
            tag: 5,
            shift: 3,
            logn: 5,
            bins: 2,
            k0: 1,
        }];
        jobs.push(ZoomJob {
            tag: 6,
            shift: 4,
            ..jobs[0]
        });
        e.submit(&jobs);
        for f in 0..n as u64 {
            let p = std::f32::consts::TAU * 4.0 * f as f32 / n as f32;
            let z = C32::from_polar(1.0, p);
            e.frame(f, &[C32::default(), z, z * 0.5, C32::default()]);
        }
        let mut out = Vec::new();
        assert_eq!(e.read(&mut out), 2);
        let (a, b) = (&out[0], &out[1]);
        assert_eq!(
            (a.tag, a.saturated, b.tag, b.saturated),
            (5, true, 6, false)
        );
        assert_eq!(b.power.len(), n);
        // point j = 4 is index n/4 + 4 of the bin's n/2
        let norm = web888_dsp::wf::window_norm(n);
        let db = |p: f32| 10.0 * (p * norm).log10();
        assert!(db(b.power[n / 4 + 4]).abs() < 0.01, "{}", db(b.power[12]));
        assert!((db(b.power[n / 2 + n / 4 + 4]) + 6.02).abs() < 0.01);
    }
}
