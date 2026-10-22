// SPDX-License-Identifier: GPL-3.0-or-later
//! Squelch (`doc/m3-multi-user.md` §5): mutes the audio while the
//! channel power stays below a threshold. Opens at the threshold, closes
//! 3 dB below it after a hang time, and fades over a few milliseconds
//! either way so it doesn't click.

/// Hysteresis below the threshold, dB.
const HYST_DB: f32 = 3.0;
/// Power averaging time constant and hang time (s).
const AVG_S: f64 = 0.02;
const HANG_S: f64 = 0.3;
const FADE_S: f64 = 0.005;

pub struct Squelch {
    threshold_db: f32,
    alpha: f32,
    power: f32,
    open: bool,
    hang: u32,
    hang_left: u32,
    step: f32,
    gain: f32,
}

impl Squelch {
    /// `threshold_db`: channel power (dBFS) that opens it, at sample
    /// rate `fs`.
    pub fn new(threshold_db: f32, fs: f64) -> Self {
        Squelch {
            threshold_db,
            alpha: (1.0 - (-1.0 / (AVG_S * fs)).exp()) as f32,
            power: 0.0,
            open: false,
            hang: (HANG_S * fs) as u32,
            hang_left: 0,
            step: (1.0 / (FADE_S * fs)) as f32,
            gain: 0.0,
        }
    }

    pub fn set_threshold(&mut self, db: f32) {
        self.threshold_db = db;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Gate `audio` by the channel power of the matching samples (`power`,
    /// |z|^2 per sample).
    pub fn process(&mut self, power: &[f32], audio: &mut [f32]) {
        let open_at = 10f32.powf(self.threshold_db / 10.0);
        let close_at = 10f32.powf((self.threshold_db - HYST_DB) / 10.0);
        for (p, a) in power.iter().zip(audio) {
            self.power += self.alpha * (p - self.power);
            if self.power >= open_at {
                self.open = true;
                self.hang_left = self.hang;
            } else if self.power < close_at {
                if self.hang_left > 0 {
                    self.hang_left -= 1;
                } else {
                    self.open = false;
                }
            }
            let want = if self.open { 1.0 } else { 0.0 };
            self.gain += (want - self.gain).clamp(-self.step, self.step);
            *a *= self.gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 12_000.0;

    #[test]
    fn opens_at_the_threshold_and_closes_after_the_hang() {
        let mut sq = Squelch::new(-80.0, FS);
        let run = |sq: &mut Squelch, db: f32, secs: f64| {
            let n = (secs * FS) as usize;
            let p = vec![10f32.powf(db / 10.0); n];
            let mut a = vec![1.0; n];
            sq.process(&p, &mut a);
            a
        };
        // noise at -90: closed, silent
        let a = run(&mut sq, -90.0, 0.5);
        assert!(!sq.is_open() && a.iter().all(|&x| x == 0.0));
        // a signal at -75 opens it within the averaging time
        let a = run(&mut sq, -75.0, 0.2);
        assert!(sq.is_open() && a[a.len() - 1] == 1.0);
        assert!(
            a[..(0.06 * FS) as usize].contains(&1.0),
            "opens within 60 ms"
        );
        // a dip to -81.5 (inside the hysteresis) keeps it open
        run(&mut sq, -81.5, 1.0);
        assert!(sq.is_open());
        // below -83: closes once the hang is over
        run(&mut sq, -90.0, 0.2);
        assert!(sq.is_open(), "hang");
        let a = run(&mut sq, -90.0, 0.3);
        assert!(!sq.is_open() && a[a.len() - 1] == 0.0);
    }
}
