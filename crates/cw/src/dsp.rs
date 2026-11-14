// SPDX-License-Identifier: GPL-3.0-or-later
//! Small DSP helpers with numpy/scipy semantics where the decoder relies
//! on them (FIR design, 'same' convolution, percentiles, a sliding
//! percentile filter), and a seeded Gaussian generator.

use num_complex::Complex32;

/// Low-pass FIR (Hamming-windowed sinc, unit DC gain), as
/// `scipy.signal.firwin(n, cutoff, fs=fs)`.
pub fn firwin(n: usize, cutoff: f64, fs: f64) -> Vec<f32> {
    let c = 2.0 * cutoff / fs; // cutoff over Nyquist
    let m = (n - 1) as f64 / 2.0;
    let mut h: Vec<f64> = (0..n)
        .map(|i| {
            let t = i as f64 - m;
            let s = if t == 0.0 {
                c
            } else {
                (std::f64::consts::PI * c * t).sin() / (std::f64::consts::PI * t)
            };
            let w = if n > 1 {
                0.54 - 0.46 * (2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64).cos()
            } else {
                1.0
            };
            s * w
        })
        .collect();
    let sum: f64 = h.iter().sum();
    for v in &mut h {
        *v /= sum;
    }
    h.into_iter().map(|v| v as f32).collect()
}

/// `np.convolve(x, h, mode="same")` for complex x and real (odd-length) h.
pub fn convolve_same(x: &[Complex32], h: &[f32]) -> Vec<Complex32> {
    let n = x.len();
    let l = h.len();
    let half = (l - 1) / 2;
    let mut y = vec![Complex32::new(0.0, 0.0); n];
    for (i, out) in y.iter_mut().enumerate() {
        // y[i] = sum_k h[k] x[i + half - k]
        let j0 = (i + half) as isize;
        let k_lo = (j0 - n as isize + 1).max(0) as usize;
        let k_hi = (j0 as usize).min(l - 1);
        let mut acc = Complex32::new(0.0, 0.0);
        for k in k_lo..=k_hi {
            acc += x[j0 as usize - k] * h[k];
        }
        *out = acc;
    }
    y
}

/// `np.convolve(p, np.ones(n) / n, mode="same")`.
pub fn smooth(p: &[f32], n: usize) -> Vec<f32> {
    let n = n.max(1);
    if n == 1 {
        return p.to_vec();
    }
    let len = p.len();
    let mut cum = vec![0.0f64; len + 1];
    for i in 0..len {
        cum[i + 1] = cum[i] + p[i] as f64;
    }
    let off = (n - 1) / 2;
    (0..len)
        .map(|i| {
            // sum of p[i + off - k], k = 0..n-1 (zero outside)
            let hi = (i + off).min(len - 1) as isize;
            let lo = (i as isize + off as isize - n as isize + 1).max(0);
            if hi < lo {
                0.0
            } else {
                ((cum[hi as usize + 1] - cum[lo as usize]) / n as f64) as f32
            }
        })
        .collect()
}

/// `np.percentile(v, q)` (linear interpolation); 0 for an empty slice.
pub fn percentile(v: &[f32], q: f64) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    percentile_sorted(&s, q)
}

pub fn percentile_sorted(s: &[f32], q: f64) -> f32 {
    let pos = q / 100.0 * (s.len() - 1) as f64;
    let i = pos.floor() as usize;
    let f = pos - i as f64;
    if i + 1 < s.len() {
        (s[i] as f64 * (1.0 - f) + s[i + 1] as f64 * f) as f32
    } else {
        s[s.len() - 1]
    }
}

pub fn median(v: &[f32]) -> f32 {
    percentile(v, 50.0)
}

/// `scipy.ndimage.percentile_filter(d, q, size, mode="nearest")`.
pub fn percentile_filter(d: &[f32], q: f64, size: usize) -> Vec<f32> {
    let n = d.len();
    if n == 0 {
        return Vec::new();
    }
    let size = size.max(1);
    let mut rank = (size as f64 * q / 100.0) as usize;
    if rank >= size {
        rank = size - 1;
    }
    let at = |j: isize| d[j.clamp(0, n as isize - 1) as usize];
    let lead = (size / 2) as isize; // window of output i: i - size/2 .. i - size/2 + size - 1
    let mut win: Vec<f32> = (0..size as isize).map(|k| at(-lead + k)).collect();
    win.sort_by(|a, b| a.total_cmp(b));
    let mut out = Vec::with_capacity(n);
    out.push(win[rank]);
    for i in 1..n as isize {
        let old = at(i - 1 - lead);
        let new = at(i - lead + size as isize - 1);
        if old.to_bits() != new.to_bits() {
            let p = win.partition_point(|v| v.total_cmp(&old).is_lt());
            win.remove(p);
            let q = win.partition_point(|v| v.total_cmp(&new).is_lt());
            win.insert(q, new);
        }
        out.push(win[rank]);
    }
    out
}

/// `np.interp(t, t[::dec], y)` with y on every dec-th sample of n.
pub fn interp_decimated(y: &[f32], dec: usize, n: usize) -> Vec<f32> {
    (0..n)
        .map(|t| {
            let k = t / dec;
            if k + 1 >= y.len() {
                return y[y.len() - 1];
            }
            let f = (t - k * dec) as f32 / dec as f32;
            y[k] + f * (y[k + 1] - y[k])
        })
        .collect()
}

/// A seeded Gaussian generator (SplitMix64 + Box-Muller).
pub struct Rng {
    s: u64,
    spare: Option<f64>,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng {
            s: seed ^ 0x9E37_79B9_7F4A_7C15,
            spare: None,
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.s = self.s.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.s;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn normal(&mut self) -> f64 {
        if let Some(v) = self.spare.take() {
            return v;
        }
        let u1 = self.uniform().max(1e-300);
        let u2 = self.uniform();
        let r = (-2.0 * u1.ln()).sqrt();
        let a = 2.0 * std::f64::consts::PI * u2;
        self.spare = Some(r * a.sin());
        r * a.cos()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firwin_unit_gain_symmetric() {
        let h = firwin(31, 40.0, 500.0);
        assert!((h.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        for i in 0..15 {
            assert!((h[i] - h[30 - i]).abs() < 1e-7);
        }
    }

    #[test]
    fn smooth_same_as_numpy() {
        // np.convolve([1,2,3,4,5], ones(2)/2, 'same') = [0.5, 1.5, 2.5, 3.5, 4.5]
        let y = smooth(&[1.0, 2.0, 3.0, 4.0, 5.0], 2);
        assert_eq!(y, vec![0.5, 1.5, 2.5, 3.5, 4.5]);
        // ones(3)/3: [1, 2, 3, 4, 3]
        let y = smooth(&[1.0, 2.0, 3.0, 4.0, 5.0], 3);
        let want = [1.0, 2.0, 3.0, 4.0, 3.0];
        for (a, b) in y.iter().zip(want) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn percentiles() {
        let v = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(percentile(&v, 50.0), 2.5);
        assert_eq!(percentile(&v, 0.0), 1.0);
        assert_eq!(percentile(&v, 100.0), 4.0);
        // percentile_filter vs brute force
        let mut r = Rng::new(3);
        let d: Vec<f32> = (0..200).map(|_| r.normal() as f32).collect();
        for &(q, size) in &[(20.0, 7usize), (90.0, 10), (97.0, 3)] {
            let f = percentile_filter(&d, q, size);
            for i in 0..d.len() {
                let mut w: Vec<f32> = (0..size)
                    .map(|k| {
                        d[(i as isize - (size / 2) as isize + k as isize).clamp(0, 199) as usize]
                    })
                    .collect();
                w.sort_by(|a, b| a.total_cmp(b));
                let rank = ((size as f64 * q / 100.0) as usize).min(size - 1);
                assert_eq!(f[i], w[rank]);
            }
        }
    }

    #[test]
    fn convolve_matches_direct() {
        let x: Vec<Complex32> = (0..10).map(|i| Complex32::new(i as f32, 0.0)).collect();
        let y = convolve_same(&x, &[0.25, 0.5, 0.25]);
        assert!((y[0].re - 0.25).abs() < 1e-6); // 0.5*0 + 0.25*1
        assert!((y[5].re - 5.0).abs() < 1e-6);
        assert!((y[9].re - (0.25 * 8.0 + 0.5 * 9.0)).abs() < 1e-6);
    }
}
