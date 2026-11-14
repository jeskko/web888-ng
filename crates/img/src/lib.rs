// SPDX-License-Identifier: GPL-3.0-or-later
//! Image modes for the web UI: HF fax ([`fax`]) and SSTV ([`sstv`]) from
//! the page's 12 kHz audio, both FM around 1900 Hz ([`fm`]). [`Imager`]
//! runs both on the same audio: fax images start on their APT start tone
//! (or by hand), SSTV images on their VIS code.

pub mod fax;
pub mod fm;
pub mod sstv;

use fax::{Fax, FaxEvent};
use fm::FmDemod;
use sstv::{Sstv, SstvEvent};

pub struct Imager {
    fm: FmDemod,
    pub fax: Fax,
    pub sstv: Sstv,
    freq: Vec<f32>,
    amp: Vec<f32>,
}

impl Default for Imager {
    fn default() -> Self {
        Self::new()
    }
}

impl Imager {
    pub fn new() -> Self {
        Imager {
            fm: FmDemod::new(),
            fax: Fax::new(),
            sstv: Sstv::new(),
            freq: Vec::new(),
            amp: Vec::new(),
        }
    }

    pub fn push(&mut self, audio: &[f32]) {
        self.freq.clear();
        self.amp.clear();
        self.fm.push(audio, &mut self.freq, &mut self.amp);
        self.fax.push(&self.freq);
        self.sstv.push(&self.freq);
    }

    pub fn poll(&mut self) -> (Vec<FaxEvent>, Vec<SstvEvent>) {
        (self.fax.poll(), self.sstv.poll())
    }
}
