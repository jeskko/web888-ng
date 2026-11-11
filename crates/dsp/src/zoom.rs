// SPDX-License-Identifier: GPL-3.0-or-later
//! Zoomed views from a bin source's zoom engine (`hw/docs/channelizer.md`
//! §3.9): a view's spectrum is a job or a few (its bins, an FFT length),
//! their results summed into the view's line. Here: how a view's bins
//! become jobs, the engine's time shared by the views ([`schedule`]), the
//! line from the results ([`ZoomAcc`]), and the shift a job's bins want.

use web888_radio::bins::{BinCaps, ZoomCaps};

/// The share of the work RAM's time the views may plan for.
pub const LOAD: f64 = 0.8;

/// A job of a spectrum: `bins` of the view's bins from index `at`, the
/// first being bin `k0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Part {
    pub at: usize,
    pub bins: usize,
    pub k0: usize,
}

/// Lengths of the runs of consecutive bin numbers (wrapping at
/// `caps.bins`) in `view` (by frequency): a job's bins are one run.
pub fn runs(caps: &BinCaps, view: &[usize]) -> Vec<usize> {
    let mut r: Vec<usize> = Vec::new();
    for (i, &k) in view.iter().enumerate() {
        match r.last_mut() {
            Some(n) if i > 0 && (view[i - 1] + 1) % caps.bins.max(1) == k => *n += 1,
            _ => r.push(1),
        }
    }
    r
}

/// Each run in as few even jobs as the engine's limits allow, as
/// `(first index, bins)`.
pub fn split(zc: &ZoomCaps, runs: &[usize], logn: u8) -> Vec<(usize, usize)> {
    let n = 1usize << logn;
    let most = zc.max_bins.min(zc.work_samples / n).max(1);
    let mut out = Vec::new();
    let mut at = 0;
    for &len in runs {
        let parts = len.div_ceil(most);
        let mut done = 0;
        for p in 0..parts {
            let b = (len * (p + 1)).div_ceil(parts) - done;
            out.push((at + done, b));
            done += b;
        }
        at += len;
    }
    out
}

/// The jobs of one spectrum of `view` (bins by frequency) at 2^logn.
pub fn plan(caps: &BinCaps, zc: &ZoomCaps, view: &[usize], logn: u8) -> Vec<Part> {
    split(zc, &runs(caps, view), logn)
        .into_iter()
        .map(|(at, bins)| Part {
            at,
            bins,
            k0: view[at],
        })
        .collect()
}

/// The work RAM's time a spectrum takes, in frames of the whole RAM: a
/// job holds its contexts for n frames.
pub fn cost(zc: &ZoomCaps, runs: &[usize], logn: u8) -> f64 {
    let n = 1usize << logn;
    split(zc, runs, logn)
        .iter()
        .map(|&(_, b)| {
            let c = zc.contexts_for(b * n).unwrap_or(zc.contexts);
            (n * c) as f64 / zc.contexts as f64
        })
        .sum()
}

/// What a view asks of the engine.
#[derive(Clone, Debug, PartialEq)]
pub struct Demand {
    /// Its bins' runs ([`runs`]).
    pub runs: Vec<usize>,
    /// Its FFT length (bins no wider than a display pixel).
    pub logn: u8,
    pub fps: f64,
    /// Most spectra a line (the FFT budget).
    pub max_a: usize,
}

/// What a view gets: `a` spectra a line of 2^logn points, at most `fps`
/// lines a second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grant {
    pub a: usize,
    pub logn: u8,
    pub fps: f64,
}

/// The engine's time shared by the views: every view the same number of
/// spectra a line (at most its own budget) within [`LOAD`] of the work
/// RAM's frames; when one each doesn't fit, every FFT one step shorter;
/// then the line rates scaled down.
pub fn schedule(zc: &ZoomCaps, frame_rate: f64, d: &[Demand]) -> Vec<Grant> {
    let budget = LOAD * frame_rate;
    let load = |step: u8| -> f64 {
        d.iter()
            .map(|v| cost(zc, &v.runs, lower(zc, v.logn, step)) * v.fps)
            .sum()
    };
    let grants = |step: u8, a: usize, r: f64| {
        d.iter()
            .map(|v| Grant {
                a: a.min(v.max_a).max(1),
                logn: lower(zc, v.logn, step),
                fps: v.fps * r,
            })
            .collect()
    };
    for step in 0..=1 {
        let t = load(step);
        if t <= 0.0 {
            return grants(step, usize::MAX, 1.0);
        }
        let a = (budget / t).floor();
        if a >= 1.0 {
            return grants(step, a.min(1e6) as usize, 1.0);
        }
    }
    grants(1, 1, budget / load(1))
}

fn lower(zc: &ZoomCaps, logn: u8, step: u8) -> u8 {
    logn.saturating_sub(step).max(zc.min_logn)
}

/// The shift a job's samples want: 3, or 4 when one of its bins reads
/// above -6 dBFS (`p`: the bin's mean |X|², a full-scale sine 1) so a
/// carrier there doesn't saturate the work RAM's 22 bits.
pub fn shift_for(p: f32) -> u8 {
    if p > 0.25 { 4 } else { 3 }
}

/// A line's spectra summed: the view's bins' middle n/2 points each.
pub struct ZoomAcc {
    half: usize,
    norm: f32,
    sum: Vec<f32>,
    count: Vec<u32>,
}

impl ZoomAcc {
    /// For `bins` view bins of n-point FFTs.
    pub fn new(bins: usize, n: usize) -> Self {
        ZoomAcc {
            half: n / 2,
            norm: crate::wf::window_norm(n),
            sum: vec![0.0; bins * n / 2],
            count: vec![0; bins],
        }
    }

    pub fn n(&self) -> usize {
        2 * self.half
    }

    /// Add a job's result, its first bin the view's `at`th.
    pub fn add(&mut self, at: usize, power: &[f32]) {
        let o = at * self.half;
        let Some(s) = self.sum.get_mut(o..o + power.len()) else {
            return;
        };
        for (s, &p) in s.iter_mut().zip(power) {
            *s += p;
        }
        let b = power.len() / self.half;
        for c in &mut self.count[at..at + b] {
            *c += 1;
        }
    }

    /// Whether bin `i` had a spectrum this line.
    pub fn has(&self, i: usize) -> bool {
        self.count.get(i).is_some_and(|&c| c > 0)
    }

    /// Bin `i`'s levels (dB, the mean of its spectra) onto `out`; false
    /// (nothing added) if it had none.
    pub fn bin_db(&self, i: usize, out: &mut Vec<f32>) -> bool {
        let c = self.count[i];
        if c == 0 {
            return false;
        }
        let k = self.norm / c as f32;
        let s = &self.sum[i * self.half..(i + 1) * self.half];
        out.extend(s.iter().map(|&p| 10.0 * (p * k).max(1e-30).log10()));
        true
    }

    /// Start the next line.
    pub fn clear(&mut self) {
        self.sum.fill(0.0);
        self.count.fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZC: ZoomCaps = ZoomCaps::ENGINE;

    fn caps(bins: usize, wraps: bool) -> BinCaps {
        BinCaps {
            bin_hz: 30e3,
            frame_rate: 60e3,
            bins,
            first_hz: 0.0,
            wraps,
            max_selected: 64,
        }
    }

    #[test]
    fn views_split_into_jobs() {
        // 480 kHz at 1024 px: 17 bins of 128 points, one job
        let v: Vec<usize> = (228..245).collect();
        let c = caps(2048, false);
        assert_eq!(
            plan(&c, &ZC, &v, 7),
            vec![Part {
                at: 0,
                bins: 17,
                k0: 228
            }]
        );
        // 7.68 MHz: 257 bins of 8 points, over 256: two jobs
        let v: Vec<usize> = (100..357).collect();
        let p = plan(&c, &ZC, &v, 3);
        assert_eq!(p.iter().map(|p| p.bins).collect::<Vec<_>>(), [129, 128]);
        assert_eq!(p[1].k0, 229);
        // 30.72 MHz: 1025 bins of 8 points: five jobs
        let v: Vec<usize> = (500..1525).collect();
        let p = plan(&c, &ZC, &v, 3);
        assert_eq!(p.iter().map(|p| p.bins).collect::<Vec<_>>(), [205; 5]);
        assert_eq!((p[4].at, p[4].k0), (820, 1320));
        // 60 kHz of 2048 points: 3 x 2048 over 4096 samples: two jobs
        let v = [235, 236, 237];
        assert_eq!(split(&ZC, &runs(&c, &v), 11), vec![(0, 2), (2, 1)]);
        // a complex input's bins cross bin 0 in one run; a gap breaks one
        let iq = caps(256, true);
        assert_eq!(runs(&iq, &[254, 255, 0, 1]), [4]);
        assert_eq!(runs(&iq, &[3, 4, 9]), [2, 1]);
    }

    #[test]
    fn a_spectrums_cost_in_work_ram_frames() {
        // 17 x 128 = 2176 samples: both contexts for 128 frames
        assert_eq!(cost(&ZC, &[17], 7), 128.0);
        // 17 x 64 = 1088: one context, two such jobs at a time
        assert_eq!(cost(&ZC, &[17], 6), 32.0);
    }

    fn views(k: usize) -> Vec<Demand> {
        vec![
            Demand {
                runs: vec![17],
                logn: 7,
                fps: 20.0,
                max_a: 6,
            };
            k
        ]
    }

    #[test]
    fn the_engine_shared_by_480_khz_views() {
        // budget 0.8 x 60 000 = 48 000 RAM frames a second; a 480 kHz view
        // at 1024 px (17 bins, n 128: both contexts) 128 a spectrum, 2560
        // a second at 20 lines/s
        let g = |k| {
            let s = schedule(&ZC, 60e3, &views(k));
            assert!(s.windows(2).all(|w| w[0] == w[1]));
            (s[0].a, s[0].logn, s[0].fps)
        };
        // 4 views: 48 000 / 10 240 = 4.7 -> 4 spectra a line
        assert_eq!(g(4), (4, 7, 20.0));
        // 8: 48 000 / 20 480 = 2.3 -> 2
        assert_eq!(g(8), (2, 7, 20.0));
        // 16: 48 000 / 40 960 = 1.2 -> 1
        assert_eq!(g(16), (1, 7, 20.0));
        // 32: 81 920 > 48 000 at one each: n 64 (17 x 64 in one context,
        // 32 frames a spectrum, 640 a second), 48 000 / 20 480 -> 2
        assert_eq!(g(32), (2, 6, 20.0));
        // 128 such views: 81 920 at n 64 and one each: the rate scaled to
        // 48 000 / 81 920 of 20, 11.7 lines/s
        let (a, logn, fps) = g(128);
        assert_eq!((a, logn), (1, 6));
        assert!((fps - 20.0 * 48_000.0 / 81_920.0).abs() < 1e-9, "{fps}");
        // a view's own budget caps its spectra
        let s = schedule(
            &ZC,
            60e3,
            &[Demand {
                max_a: 3,
                ..views(1)[0].clone()
            }],
        );
        assert_eq!(s[0].a, 3);
    }

    #[test]
    fn spectra_summed_to_db() {
        let n = 16;
        let mut a = ZoomAcc::new(3, n);
        let w = 1.0 / crate::wf::window_norm(n);
        // bins 1 and 2 in one job, twice; bin 0 none
        a.add(1, &vec![w; n]);
        a.add(1, &vec![w * 0.01; n]);
        assert!(!a.has(0) && a.has(1) && a.has(2));
        let mut db = Vec::new();
        assert!(!a.bin_db(0, &mut db));
        assert!(a.bin_db(2, &mut db));
        // (1 + 0.01) / 2
        assert_eq!(db.len(), n / 2);
        assert!((db[0] - 10.0 * 0.505f32.log10()).abs() < 1e-4, "{}", db[0]);
        a.clear();
        assert!(!a.has(1));
    }
}
