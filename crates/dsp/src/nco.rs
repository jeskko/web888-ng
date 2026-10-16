// SPDX-License-Identifier: GPL-3.0-or-later
//! Software frequency shifter (passband offset, CW pitch).

use crate::C32;
use num_complex::Complex64;

pub struct Shifter {
    phase: Complex64,
    step: Complex64,
    count: u32,
}

impl Shifter {
    /// Shift by `f_hz` at sample rate `fs` (positive moves the spectrum up).
    pub fn new(f_hz: f64, fs: f64) -> Self {
        Shifter {
            phase: Complex64::new(1.0, 0.0),
            step: Complex64::from_polar(1.0, 2.0 * std::f64::consts::PI * f_hz / fs),
            count: 0,
        }
    }

    pub fn process(&mut self, buf: &mut [C32]) {
        for x in buf {
            let p = C32::new(self.phase.re as f32, self.phase.im as f32);
            *x *= p;
            self.phase *= self.step;
            self.count += 1;
            if self.count == 1024 {
                self.count = 0;
                self.phase /= self.phase.norm();
            }
        }
    }
}
