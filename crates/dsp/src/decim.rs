// SPDX-License-Identifier: GPL-3.0-or-later
//! Integer decimation with a Kaiser lowpass (real taps, complex data),
//! computing only the kept outputs.

use crate::C32;
use crate::fir::{kaiser_len, lowpass};

pub struct Decimator {
    taps: Vec<f32>,
    r: usize,
    /// Inputs still needed: the last `taps - 1` plus any not yet used.
    buf: Vec<C32>,
    /// Index in `buf` of the newest sample of the next output's window.
    next: usize,
}

impl Decimator {
    /// Decimate by `r` at input rate `fs`, keeping `|f| <= pass_hz` flat and
    /// attenuating by `atten_db` everything that would alias into it.
    pub fn new(fs: f64, r: usize, pass_hz: f64, atten_db: f64) -> Self {
        Self::with_taps(Self::design(fs, r, pass_hz, atten_db), r)
    }

    /// Taps passing `pass_hz` and rejecting what would fold into it.
    pub fn design(fs: f64, r: usize, pass_hz: f64, atten_db: f64) -> Vec<f32> {
        assert!(r >= 1);
        let stop = fs / r as f64 - pass_hz;
        assert!(stop > pass_hz, "passband too wide for decimation by {r}");
        let n = kaiser_len(atten_db, (stop - pass_hz) / fs);
        let fc = (pass_hz + stop) / 2.0 / fs;
        lowpass(n, fc, atten_db)
            .into_iter()
            .map(|x| x as f32)
            .collect()
    }

    /// New taps, keeping the newest history (no gap in the output).
    pub fn set_taps(&mut self, taps: Vec<f32>) {
        let (old, new) = (self.taps.len(), taps.len());
        if new < old {
            let d = (old - new).min(self.next);
            self.buf.drain(..d);
            self.next -= d;
        } else {
            let d = new - old;
            self.buf
                .splice(0..0, std::iter::repeat_n(C32::new(0.0, 0.0), d));
            self.next += d;
        }
        self.taps = taps;
    }

    pub fn with_taps(taps: Vec<f32>, r: usize) -> Self {
        let n = taps.len();
        Decimator {
            taps,
            r,
            buf: vec![C32::new(0.0, 0.0); n - 1],
            next: n - 1,
        }
    }

    pub fn taps(&self) -> usize {
        self.taps.len()
    }

    /// Filter and decimate `input`, appending the outputs to `out`.
    pub fn process(&mut self, input: &[C32], out: &mut Vec<C32>) {
        self.buf.extend_from_slice(input);
        let n = self.taps.len();
        let mut i = self.next;
        while i < self.buf.len() {
            out.push(dot(&self.buf[i + 1 - n..=i], &self.taps));
            i += self.r;
        }
        let keep_from = i + 1 - n;
        self.buf.drain(..keep_from);
        self.next = i - keep_from;
    }
}

/// `x` weighed by `h`, in four partial sums: no dependency chain on one
/// accumulator (the A9's VFP waits out each add's latency otherwise).
fn dot(x: &[C32], h: &[f32]) -> C32 {
    let mut a = [C32::new(0.0, 0.0); 4];
    let (xc, xt) = x.as_chunks::<4>();
    let (hc, ht) = h.as_chunks::<4>();
    for (x, h) in xc.iter().zip(hc) {
        for k in 0..4 {
            a[k] += x[k] * h[k];
        }
    }
    let mut acc = (a[0] + a[1]) + (a[2] + a[3]);
    for (x, &h) in xt.iter().zip(ht) {
        acc += x * h;
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(f: f64, fs: f64, n: usize) -> Vec<C32> {
        (0..n)
            .map(|k| {
                let p = 2.0 * std::f64::consts::PI * f / fs * k as f64;
                C32::new(p.cos() as f32, p.sin() as f32)
            })
            .collect()
    }

    fn rms(z: &[C32]) -> f64 {
        (z.iter().map(|x| x.norm_sqr() as f64).sum::<f64>() / z.len() as f64).sqrt()
    }

    #[test]
    fn new_taps_keep_the_stream_whole() {
        let fs = 60_000.0;
        let x = tone(1500.0, fs, 60_000);
        for (a, b) in [(5300.0, 3000.0), (3000.0, 5300.0)] {
            let mut d = Decimator::new(fs, 5, a, 80.0);
            let mut y = Vec::new();
            d.process(&x[..30_001], &mut y);
            d.set_taps(Decimator::design(fs, 5, b, 80.0));
            d.process(&x[30_001..], &mut y);
            // one output per 5 inputs, the tone's level through the switch
            assert_eq!(y.len(), 12_000);
            assert!((rms(&y[5000..7000]) - 1.0).abs() < 0.01);
        }
    }

    #[test]
    fn passes_band_and_rejects_aliases() {
        let fs = 48_000.0;
        let mut d = Decimator::new(fs, 4, 5300.0, 80.0);
        assert!(d.taps() < 200, "{} taps", d.taps());
        for (f, want_db, tol) in [
            (0.0, 0.0, 0.01),
            (-5000.0, 0.0, 0.01),
            (5300.0, 0.0, 0.01),
            // would alias to -5.3 kHz and +4 kHz
            (6700.0, -80.0, f64::INFINITY),
            (-8000.0, -80.0, f64::INFINITY),
        ] {
            let mut out = Vec::new();
            // feed in odd-sized pieces: the phase must carry over
            let x = tone(f, fs, 48_000);
            for c in x.chunks(331) {
                d.process(c, &mut out);
            }
            assert_eq!(out.len(), 12_000);
            let db = 20.0 * rms(&out[1000..]).log10();
            if tol.is_finite() {
                assert!((db - want_db).abs() < tol, "{f} Hz: {db:.3} dB");
            } else {
                assert!(db < want_db, "{f} Hz: {db:.1} dB");
            }
        }
    }
}
