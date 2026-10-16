// SPDX-License-Identifier: GPL-3.0-or-later
//! Streaming overlap-save FIR filter (complex taps, complex data).

use crate::C32;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

pub struct OverlapSave {
    n: usize,
    m: usize,
    hf: Vec<C32>,
    fwd: Arc<dyn Fft<f32>>,
    inv: Arc<dyn Fft<f32>>,
    /// last m-1 inputs followed by new inputs (up to n total)
    buf: Vec<C32>,
    work: Vec<C32>,
    scratch: Vec<C32>,
}

impl OverlapSave {
    /// `taps[0]` applies to the newest sample. FFT size defaults to the next
    /// power of two >= 4 * len (at least 256).
    pub fn new(taps: &[C32]) -> Self {
        let m = taps.len();
        let n = (4 * m).next_power_of_two().max(256);
        let mut planner = FftPlanner::new();
        let fwd = planner.plan_fft_forward(n);
        let inv = planner.plan_fft_inverse(n);
        let mut hf = vec![C32::new(0.0, 0.0); n];
        hf[..m].copy_from_slice(taps);
        fwd.process(&mut hf);
        let k = 1.0 / n as f32;
        hf.iter_mut().for_each(|x| *x *= k);
        let scratch = vec![
            C32::new(0.0, 0.0);
            fwd.get_inplace_scratch_len()
                .max(inv.get_inplace_scratch_len())
        ];
        OverlapSave {
            n,
            m,
            hf,
            fwd,
            inv,
            buf: vec![C32::new(0.0, 0.0); m - 1],
            work: vec![C32::new(0.0, 0.0); n],
            scratch,
        }
    }

    /// Group delay in samples (linear-phase taps).
    pub fn delay(&self) -> usize {
        (self.m - 1) / 2
    }

    /// Filter `input`, appending outputs (one per input, delayed by block
    /// latency) to `out`.
    pub fn process(&mut self, input: &[C32], out: &mut Vec<C32>) {
        let block = self.n - (self.m - 1);
        for &x in input {
            self.buf.push(x);
            if self.buf.len() == self.n {
                self.work.copy_from_slice(&self.buf);
                self.fwd
                    .process_with_scratch(&mut self.work, &mut self.scratch);
                for (w, h) in self.work.iter_mut().zip(&self.hf) {
                    *w *= h;
                }
                self.inv
                    .process_with_scratch(&mut self.work, &mut self.scratch);
                out.extend_from_slice(&self.work[self.m - 1..]);
                self.buf.drain(..block);
            }
        }
    }
}
