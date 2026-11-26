// SPDX-License-Identifier: GPL-3.0-or-later
//! Streaming overlap-save FIR filter (complex taps, complex data).

use crate::C32;
use rustfft::Fft;
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
    /// New taps' spectrum (`set_taps`), faded in over the next block.
    next: Option<Vec<C32>>,
    old: Vec<C32>,
}

impl OverlapSave {
    /// `taps[0]` applies to the newest sample. FFT size defaults to the next
    /// power of two >= 4 * len (at least 256).
    pub fn new(taps: &[C32]) -> Self {
        Self::with_fft_size(taps, (4 * taps.len()).next_power_of_two().max(256))
    }

    /// FFT size `n` (power of two, > taps): smaller means less latency
    /// (outputs come in blocks of `n - taps + 1`) at more cost per sample.
    pub fn with_fft_size(taps: &[C32], n: usize) -> Self {
        let m = taps.len();
        assert!(n.is_power_of_two() && n > m);
        let fwd = crate::fft_plan(n, false);
        let inv = crate::fft_plan(n, true);
        let hf = Self::spectrum(taps, n, &fwd);
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
            next: None,
            old: Vec::new(),
        }
    }

    fn spectrum(taps: &[C32], n: usize, fwd: &Arc<dyn Fft<f32>>) -> Vec<C32> {
        let mut hf = vec![C32::new(0.0, 0.0); n];
        hf[..taps.len()].copy_from_slice(taps);
        fwd.process(&mut hf);
        let k = 1.0 / n as f32;
        hf.iter_mut().for_each(|x| *x *= k);
        hf
    }

    /// New taps of the same length, keeping the history: the next block's
    /// output fades from the old filter's to the new one's (no gap, no
    /// click; the delay stays). Returns false (nothing changed) if the
    /// length differs.
    pub fn set_taps(&mut self, taps: &[C32]) -> bool {
        if taps.len() != self.m {
            return false;
        }
        self.next = Some(Self::spectrum(taps, self.n, &self.fwd));
        true
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
                if let Some(next) = self.next.take() {
                    // the block through both filters, faded across it
                    self.old.clear();
                    self.old
                        .extend(self.work.iter().zip(&self.hf).map(|(w, h)| w * h));
                    self.inv
                        .process_with_scratch(&mut self.old, &mut self.scratch);
                    self.hf = next;
                    for (w, h) in self.work.iter_mut().zip(&self.hf) {
                        *w *= h;
                    }
                    self.inv
                        .process_with_scratch(&mut self.work, &mut self.scratch);
                    let k = 1.0 / block as f32;
                    for (i, (o, n)) in self.old[self.m - 1..]
                        .iter()
                        .zip(&self.work[self.m - 1..])
                        .enumerate()
                    {
                        let a = (i as f32 + 0.5) * k;
                        out.push(o * (1.0 - a) + n * a);
                    }
                } else {
                    for (w, h) in self.work.iter_mut().zip(&self.hf) {
                        *w *= h;
                    }
                    self.inv
                        .process_with_scratch(&mut self.work, &mut self.scratch);
                    out.extend_from_slice(&self.work[self.m - 1..]);
                }
                self.buf.drain(..block);
            }
        }
    }
}
