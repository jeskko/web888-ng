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
        assert!(r >= 1);
        let stop = fs / r as f64 - pass_hz;
        assert!(stop > pass_hz, "passband too wide for decimation by {r}");
        let n = kaiser_len(atten_db, (stop - pass_hz) / fs);
        let fc = (pass_hz + stop) / 2.0 / fs;
        let taps = lowpass(n, fc, atten_db)
            .into_iter()
            .map(|x| x as f32)
            .collect();
        Self::with_taps(taps, r)
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
            let w = &self.buf[i + 1 - n..=i];
            let mut acc = C32::new(0.0, 0.0);
            for (x, &h) in w.iter().zip(&self.taps) {
                acc += x * h;
            }
            out.push(acc);
            i += self.r;
        }
        let keep_from = i + 1 - n;
        self.buf.drain(..keep_from);
        self.next = i - keep_from;
    }
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
