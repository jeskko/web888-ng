// SPDX-License-Identifier: GPL-3.0-or-later
//! ADC clock estimate from GPS PPS periods (design §8).
//!
//! Each PPS period is the number of ADC clocks between two PPS edges. The
//! estimate is the trimmed mean of the last `WINDOW` plausible periods.
//! Periods far from nominal are rejected (missing or extra edges, no GPS
//! lock), as are outliers far from the current estimate. A new rate is
//! reported only when it moved by more than `retune_ppb`, so channels are
//! retuned rarely.

use std::collections::VecDeque;

const WINDOW: usize = 16;

pub struct AdcClock {
    nominal: f64,
    /// Accept periods within this many ppm of nominal.
    max_ppm: f64,
    /// Once settled, reject periods further than this from the estimate.
    outlier_ppm: f64,
    retune_ppb: f64,
    window: VecDeque<u32>,
    applied: f64,
    rejected: u64,
}

impl AdcClock {
    pub fn new(nominal: f64) -> Self {
        AdcClock {
            nominal,
            max_ppm: 100.0,
            outlier_ppm: 2.0,
            retune_ppb: 20.0,
            window: VecDeque::with_capacity(WINDOW),
            applied: nominal,
            rejected: 0,
        }
    }

    /// Rate currently in use for NCO words.
    pub fn applied(&self) -> f64 {
        self.applied
    }

    pub fn rejected(&self) -> u64 {
        self.rejected
    }

    /// Current estimate (trimmed mean), if there are enough periods.
    pub fn estimate(&self) -> Option<f64> {
        if self.window.len() < 4 {
            return None;
        }
        let mut v: Vec<u32> = self.window.iter().copied().collect();
        v.sort_unstable();
        let k = v.len() / 4;
        let mid = &v[k..v.len() - k];
        Some(mid.iter().map(|&x| x as f64).sum::<f64>() / mid.len() as f64)
    }

    /// Feed one PPS period. Returns the new rate when it should be applied.
    pub fn on_pps(&mut self, period: u32) -> Option<f64> {
        let p = period as f64;
        if ((p - self.nominal) / self.nominal).abs() > self.max_ppm * 1e-6 {
            self.rejected += 1;
            return None;
        }
        if let Some(est) = self.estimate()
            && self.window.len() >= 8
            && ((p - est) / est).abs() > self.outlier_ppm * 1e-6
        {
            self.rejected += 1;
            return None;
        }
        if self.window.len() == WINDOW {
            self.window.pop_front();
        }
        self.window.push_back(period);
        let est = self.estimate()?;
        if ((est - self.applied) / self.applied).abs() > self.retune_ppb * 1e-9 {
            self.applied = est;
            Some(est)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOM: f64 = 122_880_000.0;

    #[test]
    fn converges_with_jitter_and_rejects_bad_periods() {
        let truth = NOM * (1.0 + 2.5e-6); // 122 880 307.2
        let mut c = AdcClock::new(NOM);
        let mut applied = Vec::new();
        for k in 0..40u32 {
            // integer periods dither around the true value
            let p = if k % 5 == 0 {
                truth.ceil()
            } else {
                truth.floor()
            } as u32;
            if let Some(r) = c.on_pps(p) {
                applied.push(r);
            }
            if k == 20 {
                assert_eq!(c.on_pps(NOM as u32 * 2), None); // missed edge
                assert_eq!(c.on_pps(p + 5000), None); // glitch, 40 ppm
            }
        }
        assert_eq!(c.rejected(), 2);
        let est = c.estimate().unwrap();
        assert!(((est - truth) / truth).abs() < 2e-9, "{est} vs {truth}");
        // applied once early (2.5 ppm away), then only small moves
        assert!(!applied.is_empty() && applied.len() < 5, "{applied:?}");
        assert!(((c.applied() - truth) / truth).abs() < 25e-9);
    }

    #[test]
    fn needs_a_few_periods() {
        let mut c = AdcClock::new(NOM);
        for _ in 0..3 {
            assert_eq!(c.on_pps(NOM as u32 + 300), None);
        }
        assert!(c.on_pps(NOM as u32 + 300).is_some());
    }
}
