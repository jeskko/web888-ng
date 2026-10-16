// SPDX-License-Identifier: GPL-3.0-or-later
//! RF scene for the simulator. Levels are in dBFS relative to a full-scale
//! real sine at the ADC.

/// LTC2208 typical ADC noise floor: SNR ~77.7 dBFS over 61.44 MHz.
pub const ADC_NOISE_DBFS_HZ: f64 = -155.6;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Source {
    /// Real sine at the antenna input.
    Tone { f_hz: f64, dbfs: f64, phase: f64 },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    pub sources: Vec<Source>,
    /// White noise density at the ADC (dBFS/Hz), `None` = noiseless.
    pub noise_dbfs_hz: Option<f64>,
}

impl Scene {
    pub fn new() -> Self {
        Scene::default()
    }

    pub fn tone(mut self, f_hz: f64, dbfs: f64) -> Self {
        self.sources.push(Source::Tone {
            f_hz,
            dbfs,
            phase: 0.0,
        });
        self
    }

    /// AM carrier at `fc` with a sine modulation at `fm` and `depth` (0..1):
    /// carrier plus two sidebands at depth/2 each.
    pub fn am(mut self, fc: f64, dbfs: f64, fm: f64, depth: f64) -> Self {
        let sb = dbfs + 20.0 * (depth / 2.0).log10();
        self.sources.push(Source::Tone {
            f_hz: fc,
            dbfs,
            phase: 0.0,
        });
        self.sources.push(Source::Tone {
            f_hz: fc - fm,
            dbfs: sb,
            phase: 0.0,
        });
        self.sources.push(Source::Tone {
            f_hz: fc + fm,
            dbfs: sb,
            phase: 0.0,
        });
        self
    }

    pub fn noise(mut self, dbfs_hz: f64) -> Self {
        self.noise_dbfs_hz = Some(dbfs_hz);
        self
    }

    /// Typical ADC noise floor.
    pub fn adc_noise(self) -> Self {
        self.noise(ADC_NOISE_DBFS_HZ)
    }
}
