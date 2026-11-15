// SPDX-License-Identifier: GPL-3.0-or-later
//! JS8's submodes: the same 79-symbol frame at different symbol lengths
//! (the tone spacing is the symbol rate), each in its own UTC-aligned
//! period.

use web888_ftx::js8::{COSTAS_MODIFIED, COSTAS_NORMAL};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Submode {
    Normal,
    Fast,
    Turbo,
    Slow,
}

impl Submode {
    pub const ALL: [Submode; 4] = [
        Submode::Normal,
        Submode::Fast,
        Submode::Turbo,
        Submode::Slow,
    ];

    /// Samples a symbol at 12 kS/s.
    pub fn nsps(self) -> usize {
        match self {
            Submode::Normal => 1920,
            Submode::Fast => 1200,
            Submode::Turbo => 600,
            Submode::Slow => 3840,
        }
    }

    /// Period (s): transmissions start this far apart, on UTC multiples.
    pub fn period(self) -> u32 {
        match self {
            Submode::Normal => 15,
            Submode::Fast => 10,
            Submode::Turbo => 6,
            Submode::Slow => 30,
        }
    }

    /// Nominal start of a transmission in its period (s).
    pub fn start(self) -> f32 {
        match self {
            Submode::Normal | Submode::Slow => 0.5,
            Submode::Fast => 0.2,
            Submode::Turbo => 0.1,
        }
    }

    pub fn costas(self) -> [[u8; 7]; 3] {
        if self == Submode::Normal {
            COSTAS_NORMAL
        } else {
            COSTAS_MODIFIED
        }
    }

    /// Symbol rate and tone spacing (Hz).
    pub fn baud(self) -> f32 {
        12_000.0 / self.nsps() as f32
    }

    /// Normal's symbol length over this one's: how much this submode's
    /// audio is stretched to look like Normal.
    pub fn stretch(self) -> f64 {
        1920.0 / self.nsps() as f64
    }

    pub fn name(self) -> &'static str {
        match self {
            Submode::Normal => "normal",
            Submode::Fast => "fast",
            Submode::Turbo => "turbo",
            Submode::Slow => "slow",
        }
    }
}
