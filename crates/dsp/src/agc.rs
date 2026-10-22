// SPDX-License-Identifier: GPL-3.0-or-later
//! Audio AGC: a peak follower with fast attack, hang time and a slow,
//! constant-rate (dB/s) decay.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AgcSpec {
    /// Output peak level the AGC aims for (full scale 1.0).
    pub target: f32,
    pub max_gain_db: f32,
    pub attack_s: f64,
    /// Time the gain is held after the level drops (0 for AM).
    pub hang_s: f64,
    /// Gain recovery after the hang time.
    pub decay_db_s: f64,
}

impl AgcSpec {
    /// SSB/CW: 300 ms hang.
    pub fn ssb() -> Self {
        AgcSpec {
            target: 0.3,
            max_gain_db: 100.0,
            attack_s: 0.001,
            hang_s: 0.3,
            decay_db_s: 20.0,
        }
    }

    /// AM: no hang (the carrier holds the level).
    pub fn am() -> Self {
        AgcSpec {
            hang_s: 0.0,
            ..Self::ssb()
        }
    }
}

pub struct Agc {
    target: f32,
    min_env: f32,
    attack: f32,
    decay: f32,
    hang: u32,
    hang_left: u32,
    env: f32,
}

impl Agc {
    pub fn new(spec: AgcSpec, fs: f64) -> Self {
        let min_env = spec.target / 10f32.powf(spec.max_gain_db / 20.0);
        Agc {
            target: spec.target,
            min_env,
            attack: (1.0 - (-1.0 / (spec.attack_s * fs)).exp()) as f32,
            decay: 10f64.powf(-spec.decay_db_s / 20.0 / fs) as f32,
            hang: (spec.hang_s * fs) as u32,
            hang_left: 0,
            env: min_env,
        }
    }

    /// Current gain in dB.
    pub fn gain_db(&self) -> f32 {
        20.0 * (self.target / self.env).log10()
    }

    /// Apply the AGC in place; the output is clipped to +-1.
    pub fn process(&mut self, buf: &mut [f32]) {
        for x in buf {
            let a = x.abs();
            if a > self.env {
                self.env += self.attack * (a - self.env);
                self.hang_left = self.hang;
            } else if self.hang_left > 0 {
                self.hang_left -= 1;
            } else {
                self.env = (self.env * self.decay).max(self.min_env);
            }
            *x = (*x * self.target / self.env).clamp(-1.0, 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 12_000.0;

    fn tone(amp: f32, secs: f64, k0: usize) -> Vec<f32> {
        let n = (secs * FS) as usize;
        (k0..k0 + n)
            .map(|k| amp * (2.0 * std::f32::consts::PI * 1000.0 / FS as f32 * k as f32).sin())
            .collect()
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0, |m, v| m.max(v.abs()))
    }

    #[test]
    fn levels_attack_hang_and_decay() {
        let s = AgcSpec::ssb();
        let mut agc = Agc::new(s, FS);

        // weak signal: settles to the target
        let mut x = tone(1e-3, 2.0, 0);
        agc.process(&mut x);
        let p = peak(&x[x.len() - 1200..]);
        assert!((p - s.target).abs() < 0.02 * s.target, "{p}");

        // 40 dB step up: within 10 ms the output is back near the target
        let mut x = tone(0.1, 0.5, 24_000);
        agc.process(&mut x);
        assert!(peak(&x[120..]) < 1.1 * s.target, "{}", peak(&x[120..]));
        let g_loud = agc.gain_db();
        assert!((g_loud - 20.0 * (s.target / 0.1).log10()).abs() < 0.5);

        // drop back: the gain holds for the hang time, then rises at 20 dB/s
        let mut x = tone(1e-3, 0.25, 30_000);
        agc.process(&mut x);
        assert!((agc.gain_db() - g_loud).abs() < 0.1, "held during hang");
        let mut x = tone(1e-3, 1.05, 33_000);
        agc.process(&mut x);
        let rose = agc.gain_db() - g_loud;
        assert!((rose - 20.0).abs() < 1.0, "rose {rose} dB in 1 s");
    }

    #[test]
    fn max_gain_limits_silence() {
        let mut agc = Agc::new(
            AgcSpec {
                max_gain_db: 40.0,
                ..AgcSpec::am()
            },
            FS,
        );
        let mut x = vec![0.0; 120_000];
        agc.process(&mut x);
        assert!((agc.gain_db() - 40.0).abs() < 1e-3);
    }
}
