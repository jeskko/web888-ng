// SPDX-License-Identifier: GPL-3.0-or-later
//! The RX stream's sample counter to UTC (`doc/api.md` §8.5).
//!
//! The hardware numbers its frames but doesn't timestamp them. Each block
//! is read at a host clock time, late by the transfer and scheduling
//! delay; the least late block of each second is the best estimate of
//! when its frames were made. A line fitted through those minima over the
//! last minutes gives every frame a UTC time, without the reads' jitter,
//! and its slope the stream's real rate against the host clock (the ADC
//! clock's error, measured against whatever disciplines the host clock).
//!
//! What the host clock is worth comes from outside ([`Timebase::set_system`]:
//! the kernel's synchronisation state), as later a time-code station or
//! GPS will. Left without a reference the clock carries on (`holdover`)
//! and the uncertainty grows.

use std::collections::VecDeque;

/// Where the time comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The host clock, synchronised (NTP, or chrony with any source).
    Ntp,
    /// It was, and isn't now: carried on by the local oscillators.
    Holdover,
    /// Never synchronised since the start.
    None,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::Ntp => "ntp",
            Source::Holdover => "holdover",
            Source::None => "none",
        }
    }
}

/// What the timebase says of itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimeInfo {
    pub source: Source,
    /// One standard uncertainty of a sample's UTC (s).
    pub uncertainty: f64,
    /// The stream's rate against the host clock, ppm from nominal (the
    /// ADC clock's error when the host clock is good); none until the
    /// fit spans a minute.
    pub adc_ppm: Option<f64>,
}

/// Frames per bucket: about a second's.
fn bucket_frames(rate: f64) -> u64 {
    rate.round().max(1.0) as u64
}

/// Buckets kept: ten minutes.
const KEEP: usize = 600;
/// The fit's points before it is used.
const MIN_POINTS: usize = 5;
/// A jump of the offset beyond this is a new time base (the host clock
/// stepped, the stream restarted).
const JUMP_S: f64 = 0.5;
/// The transfer's least delay isn't known: until a time-code station or
/// GPS measures it, this much uncertainty is allowed for it (s).
const LATENCY_ALLOWANCE: f64 = 1e-3;
/// The host clock's error when its daemon doesn't say (s): an NTP
/// client over the internet is typically within a few ms.
const UNREPORTED: f64 = 5e-3;
/// Holdover: how fast the uncertainty grows (the oscillator's drift
/// against the last reference, s/s; ~ the host clock's unsteered
/// stability).
const HOLDOVER_RATE: f64 = 1e-6;

#[derive(Clone, Copy, Debug)]
struct Fit {
    /// resid(frame) = a + b * (frame - f_ref)
    f_ref: f64,
    a: f64,
    b: f64,
    sigma: f64,
    span: f64,
}

#[derive(Debug, Default)]
pub struct Timebase {
    /// Nominal frames per second (0: none seen yet).
    nominal: f64,
    /// The current bucket: its number and least (frame, offset).
    cur: Option<(u64, f64, f64)>,
    /// Finished buckets' least (frame, offset): offset = arrival - frame / nominal.
    pts: VecDeque<(f64, f64)>,
    fit: Option<Fit>,
    /// The least offset seen since the last reset (before the fit).
    least: Option<f64>,
    synced: bool,
    ever_synced: bool,
    /// The host clock's estimated error when synchronised (s).
    esterror: f64,
    /// Host time when the reference was last good (s).
    last_sync: f64,
    /// Host time of the last block.
    now: f64,
}

impl Timebase {
    /// A block ending at frame `end` (frames numbered since the stream
    /// started, at `nominal` per second) was read at host time `now`.
    pub fn observe(&mut self, end: u64, now: f64, nominal: f64) {
        if nominal <= 0.0 || now <= 0.0 {
            return;
        }
        if self.nominal == 0.0 || (nominal / self.nominal - 1.0).abs() > 1e-3 {
            self.reset(nominal);
        }
        self.now = now;
        let f = end as f64;
        let o = now - f / self.nominal;
        // earlier than the clock allows (it stepped back, or the frames
        // restarted from a later point): a new time base
        if self.offset_at(f).is_some_and(|p| o < p - JUMP_S) {
            self.reset(nominal);
        }
        let k = end / bucket_frames(self.nominal);
        match &mut self.cur {
            Some((b, bf, bo)) if *b == k => {
                if o < *bo {
                    *bf = f;
                    *bo = o;
                }
            }
            _ => {
                if let Some((_, bf, bo)) = self.cur.take() {
                    // a whole second late: the clock stepped forward, or
                    // the frames restarted from 0
                    if self.offset_at(bf).is_some_and(|p| bo > p + JUMP_S) {
                        self.reset(nominal);
                    }
                    self.least = Some(self.least.map_or(bo, |l| l.min(bo)));
                    self.pts.push_back((bf, bo));
                    while self.pts.len() > KEEP {
                        self.pts.pop_front();
                    }
                    self.refit();
                }
                self.cur = Some((k, f, o));
            }
        }
    }

    fn reset(&mut self, nominal: f64) {
        self.nominal = nominal;
        self.cur = None;
        self.pts.clear();
        self.fit = None;
        self.least = None;
    }

    /// Least squares through the buckets' minima; then again without the
    /// ones well above the line (buckets whose every block was late).
    fn refit(&mut self) {
        if self.pts.len() < MIN_POINTS {
            self.fit = None;
            return;
        }
        let f_ref = self.pts.back().map_or(0.0, |p| p.0);
        let mut use_: Vec<(f64, f64)> = self.pts.iter().map(|&(f, o)| (f - f_ref, o)).collect();
        let mut fit = None;
        for _ in 0..2 {
            let Some((a, b)) = line(&use_) else { return };
            let r: Vec<f64> = use_.iter().map(|&(x, y)| y - (a + b * x)).collect();
            let mut s: Vec<f64> = r.iter().map(|v| v.abs()).collect();
            s.sort_by(f64::total_cmp);
            let mad = s[s.len() / 2].max(1e-6);
            let sigma = (r.iter().map(|v| v * v).sum::<f64>() / r.len() as f64).sqrt();
            let span = (use_.last().map_or(0.0, |p| p.0) - use_.first().map_or(0.0, |p| p.0)).abs();
            fit = Some(Fit {
                f_ref,
                a,
                b,
                sigma,
                span,
            });
            let keep: Vec<(f64, f64)> = use_
                .iter()
                .zip(&r)
                .filter(|&(_, &v)| v < 3.0 * 1.4826 * mad)
                .map(|(&p, _)| p)
                .collect();
            if keep.len() < MIN_POINTS || keep.len() == use_.len() {
                break;
            }
            use_ = keep;
        }
        self.fit = fit;
    }

    /// The offset (UTC - frame / nominal) at frame `f`.
    fn offset_at(&self, f: f64) -> Option<f64> {
        match self.fit {
            Some(t) => Some(t.a + t.b * (f - t.f_ref)),
            None => match (self.least, self.cur.map(|c| c.2)) {
                (Some(l), Some(c)) => Some(l.min(c)),
                (l, c) => l.or(c),
            },
        }
    }

    /// UTC (s) of frame `frame` (fractional: a point between frames).
    pub fn utc(&self, frame: f64) -> Option<f64> {
        if self.nominal == 0.0 {
            return None;
        }
        self.offset_at(frame).map(|o| o + frame / self.nominal)
    }

    /// The host clock lately, as one point (frame, host time): the last
    /// `n` seconds' least-late reads, each moved to their mean frame with
    /// the stream taken to run at `rate` frames per second of true time
    /// (a reference's rate, not the host clock's). For measuring the host
    /// clock while something steers it: [`Timebase::utc`]'s fit spans
    /// minutes, so against a clock being slewed it lags by minutes, and a
    /// loop closed over it oscillates (the LF-only chrony trial of
    /// 2026-11-20). The point is as old as the reads' middle.
    pub fn recent(&self, n: usize, rate: f64) -> Option<(f64, f64)> {
        let n = n.max(MIN_POINTS);
        if self.nominal == 0.0 || rate <= 0.0 || self.pts.len() < MIN_POINTS {
            return None;
        }
        let p: Vec<(f64, f64)> = self
            .pts
            .iter()
            .rev()
            .take(n)
            .map(|&(f, o)| (f, o + f / self.nominal))
            .collect();
        let fa = p.iter().map(|x| x.0).sum::<f64>() / p.len() as f64;
        let mut h: Vec<f64> = p.iter().map(|&(f, t)| t + (fa - f) / rate).collect();
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        // as the fit: again without the reads well above the rest
        for _ in 0..2 {
            let m = mean(&h);
            let mut s: Vec<f64> = h.iter().map(|v| (v - m).abs()).collect();
            s.sort_by(f64::total_cmp);
            let mad = s[s.len() / 2].max(1e-6);
            let keep: Vec<f64> = h
                .iter()
                .copied()
                .filter(|v| v - m < 3.0 * 1.4826 * mad)
                .collect();
            if keep.len() < MIN_POINTS || keep.len() == h.len() {
                break;
            }
            h = keep;
        }
        Some((fa, mean(&h)))
    }

    /// The stream's frames per second by the host clock.
    pub fn rate(&self) -> Option<f64> {
        if self.nominal == 0.0 {
            return None;
        }
        Some(match self.fit {
            Some(t) => 1.0 / (1.0 / self.nominal + t.b),
            None => self.nominal,
        })
    }

    /// The host clock's state: synchronised or not, and its estimated
    /// error (s), at host time `now`.
    pub fn set_system(&mut self, synced: bool, esterror: f64, now: f64) {
        self.synced = synced;
        if synced {
            self.ever_synced = true;
            // timesyncd leaves it at 0: not "exact", just not said
            self.esterror = if esterror > 0.0 { esterror } else { UNREPORTED };
            self.last_sync = now;
        }
    }

    pub fn info(&self) -> TimeInfo {
        let fit_sigma = self.fit.map_or(LATENCY_ALLOWANCE, |f| f.sigma);
        let (source, clock) = if self.synced {
            (Source::Ntp, self.esterror)
        } else if self.ever_synced {
            let since = (self.now - self.last_sync).max(0.0);
            (Source::Holdover, self.esterror + HOLDOVER_RATE * since)
        } else {
            (Source::None, f64::INFINITY)
        };
        let uncertainty =
            (clock * clock + fit_sigma * fit_sigma + LATENCY_ALLOWANCE * LATENCY_ALLOWANCE).sqrt();
        // a slope over less than a minute says little
        let adc_ppm = self
            .fit
            .filter(|f| f.span >= 60.0 * self.nominal)
            .and_then(|_| self.rate())
            .map(|r| (r / self.nominal - 1.0) * 1e6);
        TimeInfo {
            source,
            uncertainty,
            adc_ppm,
        }
    }
}

/// Least-squares line y = a + b x.
fn line(p: &[(f64, f64)]) -> Option<(f64, f64)> {
    let n = p.len() as f64;
    let (sx, sy) = p.iter().fold((0.0, 0.0), |s, &(x, y)| (s.0 + x, s.1 + y));
    let (mx, my) = (sx / n, sy / n);
    let (sxx, sxy) = p.iter().fold((0.0, 0.0), |s, &(x, y)| {
        (s.0 + (x - mx) * (x - mx), s.1 + (x - mx) * (y - my))
    });
    if sxx <= 0.0 {
        return None;
    }
    let b = sxy / sxx;
    Some((my - b * mx, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Blocks of 256 frames from a stream `ppm` off nominal, read late by
    /// 0.2-5 ms (a few blocks much later), host clock from `t0`.
    fn feed(tb: &mut Timebase, t0: f64, ppm: f64, secs: f64) {
        let nominal = 48_000.0;
        let real = nominal * (1.0 + ppm * 1e-6);
        let mut end = 0u64;
        let mut k = 0u64;
        while (end as f64) < secs * real {
            end += 256;
            k += 1;
            // deterministic "random" lateness
            let late = 0.0002
                + 0.0048 * ((k * 7919 % 1000) as f64 / 1000.0)
                + if k.is_multiple_of(97) { 0.05 } else { 0.0 };
            tb.observe(end, t0 + end as f64 / real + late, nominal);
        }
    }

    #[test]
    fn frames_get_their_time_and_the_rate_its_error() {
        let mut tb = Timebase::default();
        assert_eq!(tb.utc(0.0), None);
        let t0 = 1.8e9;
        feed(&mut tb, t0, 0.31, 300.0);
        let real = 48_000.0 * (1.0 + 0.31e-6);
        // the least lateness (0.2 ms) is what remains
        for f in [0.0, 1e6, 1.4e7] {
            let want = t0 + f / real + 0.0002;
            let got = tb.utc(f).unwrap();
            assert!(
                (got - want).abs() < 50e-6,
                "frame {f}: {} us",
                (got - want) * 1e6
            );
        }
        let i = tb.info();
        assert_eq!(i.source, Source::None);
        assert!((i.adc_ppm.unwrap() - 0.31).abs() < 0.05, "{:?}", i.adc_ppm);
        tb.set_system(true, 0.002, t0 + 300.0);
        assert_eq!(tb.info().source, Source::Ntp);
        let i = tb.info();
        assert_eq!(i.source, Source::Ntp);
        assert!(
            i.uncertainty > 0.002 && i.uncertainty < 0.003,
            "{}",
            i.uncertainty
        );
        // unsynchronised later: holdover, growing
        tb.set_system(false, 0.0, t0 + 300.0);
        feed(&mut tb, t0, 0.31, 1300.0);
        let i = tb.info();
        assert_eq!(i.source, Source::Holdover);
        assert!(i.uncertainty > 0.002 + 0.9e-3, "{}", i.uncertainty);
    }

    #[test]
    fn a_stepped_clock_or_a_restarted_stream_starts_over() {
        let mut tb = Timebase::default();
        feed(&mut tb, 1.8e9, 0.0, 30.0);
        assert!(tb.fit.is_some());
        // the stream restarts: frame numbers from 0 again, an hour later
        feed(&mut tb, 1.8e9 + 3600.0, 0.0, 10.0);
        let got = tb.utc(0.0).unwrap();
        assert!(
            (got - (1.8e9 + 3600.0 + 0.0002)).abs() < 1e-3,
            "{}",
            got - 1.8e9
        );
        // the host clock steps back a minute
        feed(&mut tb, 1.8e9 + 3540.0, 0.0, 10.0);
        let got = tb.utc(0.0).unwrap();
        assert!(
            (got - (1.8e9 + 3540.0 + 0.0002)).abs() < 1e-3,
            "{}",
            got - 1.8e9
        );
        // another nominal rate: a new stream
        tb.observe(256, 1.9e9, 24_000.0);
        assert_eq!(tb.pts.len(), 0);
    }

    /// A clock steered by a simple PI loop (a chrony stand-in) from the
    /// offset it measures each 64 s through the timebase, the stream's
    /// rate known (as from a station's carrier): the host clock's error
    /// (s) each minute over `secs`.
    fn steer(recent: bool, secs: f64) -> Vec<f64> {
        let (nominal, block) = (48_000.0, 480u64);
        let real = nominal * (1.0 + 0.4e-6);
        let t0 = 1.8e9;
        let mut tb = Timebase::default();
        // the host clock's error (s), its oscillator's (s/s), the loop's
        // frequency correction and phase slew
        let (mut x, f_err, mut f_corr, mut slew) = (0.005, 20e-6, 0.0, 0.0);
        let (mut end, mut k, mut next) = (0u64, 0u64, 64.0);
        let mut out = Vec::new();
        let dt = block as f64 / real;
        while (end as f64) / real < secs {
            end += block;
            k += 1;
            x += (f_err + f_corr + slew) * dt;
            let t = end as f64 / real;
            let late = 0.0048 * ((k * 7919 % 1000) as f64 / 1000.0);
            tb.observe(end, t0 + t + x + late, nominal);
            if t >= next {
                next += 64.0;
                // true minus host time, as the stations give it
                let o = if recent {
                    tb.recent(20, real).map(|(fa, sa)| t0 + fa / real - sa)
                } else {
                    let f = end as f64 - 30.0 * real;
                    tb.utc(f).map(|u| t0 + f / real - u)
                };
                if let Some(o) = o {
                    slew = 0.5 * o / 64.0;
                    f_corr += 0.1 * o / 64.0;
                }
            }
            if k % (60 * real as u64 / block) == 0 {
                out.push(x);
            }
        }
        out
    }

    /// Measured through the fit (minutes long) the loop swings ever wider,
    /// as the 2026-11-20 trial did; through the recent reads it settles.
    #[test]
    fn a_loop_over_the_recent_reads_settles() {
        let worst = |v: &[f64]| v[v.len() - 30..].iter().fold(0.0f64, |m, x| m.max(x.abs()));
        let r = steer(true, 7200.0);
        let f = steer(false, 7200.0);
        eprintln!(
            "recent {:.3} ms, fit {:.3} ms",
            worst(&r) * 1e3,
            worst(&f) * 1e3
        );
        assert!(worst(&r) < 0.3e-3, "{:?}", r);
        assert!(worst(&f) > 10.0 * worst(&r), "{:?}", f);
    }
}
