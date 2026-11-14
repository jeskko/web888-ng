// SPDX-License-Identifier: GPL-3.0-or-later
//! HF fax (WEFAX, as DWD and NOAA send weather charts): FM 1500 Hz black
//! to 2300 Hz white around 1900 Hz (dial = carrier − 1.9 kHz in USB).
//!
//! * APT: the start tone (black/white at 300 Hz for IOC 576, 675 Hz for
//!   IOC 288) begins an image, the stop tone (450 Hz) ends it; found as
//!   the share of the luminance's variance at that frequency over 0.25 s
//!   windows (one second of it).
//! * Phasing: the 30 s after the start tone are lines with a narrow
//!   pulse (5 % of a line) at their start. Its position line by line gives
//!   the line start and, from its drift, the real line period: the audio
//!   clock is a few hundred ppm off, which slants an image by a quarter of
//!   its width without this.
//! * Lines are rendered as soon as their samples are in; the image is
//!   redrawn when phasing sets the timing, and on a manual slant or shift
//!   (the luminance since the image start is kept).
//! * Without APT (tuned in mid-chart) an image is started by hand.

use crate::fm::FM_RATE;

/// Lines per minute (DWD, NOAA: 120).
pub const LPM: f64 = 120.0;
/// Images longer than this are cut (lines).
pub const MAX_LINES: usize = 3000;
const WIN: usize = (0.25 * FM_RATE) as usize;
const PHASING_LINES: usize = 40;

/// What the decoder reports.
#[derive(Clone, Debug, PartialEq)]
pub enum FaxEvent {
    /// A new image `width` pixels wide (`auto`: from APT).
    Image { width: usize, ioc: u32, auto: bool },
    /// Rows `first..` (width bytes each, 0 black .. 255 white).
    Rows { first: usize, data: Vec<u8> },
    /// Everything is drawn again: rows from 0 follow.
    Redraw,
    /// The image ended (stop tone, by hand, or too long).
    Done { lines: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaxState {
    Idle,
    /// The start tone is on.
    Start,
    Image,
}

pub struct Fax {
    pub state: FaxState,
    pub ioc: u32,
    pub lpm: f64,
    /// FM samples in.
    n: u64,
    /// Luminance (0..1) since `lum_t0`.
    lum: Vec<f32>,
    lum_t0: u64,
    /// APT windows: (tone 0 = 300, 1 = 675, 2 = 450) run lengths.
    win: Vec<f32>,
    runs: [u32; 3],
    quiet: u32,
    /// Line timing (FM samples): line k starts at start + k * period.
    start: f64,
    period: f64,
    /// Fitted (or nominal) period before the manual slant.
    period_fit: f64,
    /// Manual slant (ppm) and shift (fraction of a line).
    slant_ppm: f64,
    shift: f64,
    phased: bool,
    rows: usize,
    events: Vec<FaxEvent>,
}

impl Default for Fax {
    fn default() -> Self {
        Self::new()
    }
}

/// Luminance of an FM frequency.
pub fn lum_of(f: f32) -> f32 {
    ((f - 1500.0) / 800.0).clamp(0.0, 1.0)
}

/// The share of x's variance at frequency f (Hz at FM_RATE): ~0.8 for the
/// APT square wave, small for anything else.
fn tone_share(x: &[f32], f: f64) -> f64 {
    let n = x.len() as f64;
    let mean = x.iter().map(|&v| v as f64).sum::<f64>() / n;
    let ss: f64 = x.iter().map(|&v| (v as f64 - mean).powi(2)).sum();
    if ss < 0.01 * n {
        return 0.0; // no swing: not a tone
    }
    let w = 2.0 * std::f64::consts::PI * f / FM_RATE;
    let (mut re, mut im) = (0.0, 0.0);
    for (i, &v) in x.iter().enumerate() {
        let a = w * i as f64;
        re += (v as f64 - mean) * a.cos();
        im -= (v as f64 - mean) * a.sin();
    }
    (re * re + im * im) / (n * ss / 2.0)
}

impl Fax {
    pub fn new() -> Self {
        Fax {
            state: FaxState::Idle,
            ioc: 576,
            lpm: LPM,
            n: 0,
            lum: Vec::new(),
            lum_t0: 0,
            win: Vec::new(),
            runs: [0; 3],
            quiet: 0,
            start: 0.0,
            period: 0.0,
            period_fit: 0.0,
            slant_ppm: 0.0,
            shift: 0.0,
            phased: false,
            rows: 0,
            events: Vec::new(),
        }
    }

    pub fn width(&self) -> usize {
        (self.ioc as f64 * std::f64::consts::PI).round() as usize
    }

    fn nominal_period(&self) -> f64 {
        60.0 / self.lpm * FM_RATE
    }

    /// The slant (ppm) the timing has relative to the nominal line period.
    pub fn slant(&self) -> f64 {
        if self.period == 0.0 {
            0.0
        } else {
            (self.period / self.nominal_period() - 1.0) * 1e6
        }
    }

    pub fn lines(&self) -> usize {
        self.rows
    }

    pub fn poll(&mut self) -> Vec<FaxEvent> {
        std::mem::take(&mut self.events)
    }

    /// Feed FM frequencies (Hz).
    pub fn push(&mut self, freq: &[f32]) {
        for &f in freq {
            let l = lum_of(f);
            self.lum.push(l);
            self.win.push(l);
            self.n += 1;
            if self.win.len() == WIN {
                let w = std::mem::take(&mut self.win);
                self.apt(&w);
            }
        }
        if self.state == FaxState::Image {
            self.render_new();
        } else {
            // keep a little (the start tone's end decides the image start)
            let keep = (2.0 * FM_RATE) as usize;
            if self.lum.len() > 2 * keep {
                let cut = self.lum.len() - keep;
                self.lum.drain(..cut);
                self.lum_t0 += cut as u64;
            }
        }
    }

    fn apt(&mut self, w: &[f32]) {
        let shares = [
            tone_share(w, 300.0),
            tone_share(w, 675.0),
            tone_share(w, 450.0),
        ];
        for (r, s) in self.runs.iter_mut().zip(shares) {
            *r = if s > 0.4 { *r + 1 } else { 0 };
        }
        let win_end = self.n;
        match self.state {
            FaxState::Idle | FaxState::Image if self.runs[0] >= 4 || self.runs[1] >= 4 => {
                if self.state == FaxState::Image {
                    self.finish();
                }
                self.ioc = if self.runs[0] >= 4 { 576 } else { 288 };
                self.state = FaxState::Start;
                self.quiet = 0;
            }
            FaxState::Start => {
                if self.runs[0] == 0 && self.runs[1] == 0 {
                    self.quiet += 1;
                    if self.quiet >= 2 {
                        // the tone ended two windows ago
                        let t = win_end as f64 - 2.0 * WIN as f64;
                        self.begin(t, true);
                    }
                } else {
                    self.quiet = 0;
                }
            }
            FaxState::Image if self.runs[2] >= 4 => {
                self.finish();
            }
            _ => {}
        }
    }

    /// Start an image at FM sample `t`.
    fn begin(&mut self, t: f64, auto: bool) {
        let cut = ((t as u64).saturating_sub(self.lum_t0) as usize).min(self.lum.len());
        self.lum.drain(..cut);
        self.lum_t0 += cut as u64;
        self.state = FaxState::Image;
        self.period_fit = self.nominal_period();
        self.period = self.period_fit * (1.0 + self.slant_ppm * 1e-6);
        self.start = self.lum_t0 as f64;
        self.phased = !auto;
        self.rows = 0;
        self.events.push(FaxEvent::Image {
            width: self.width(),
            ioc: self.ioc,
            auto,
        });
    }

    /// Start an image now (no APT heard).
    pub fn start_now(&mut self) {
        if self.state == FaxState::Image {
            self.finish();
        }
        self.begin(self.n as f64, false);
    }

    /// End the image now.
    pub fn stop_now(&mut self) {
        if self.state == FaxState::Image {
            self.finish();
        }
        self.state = FaxState::Idle;
    }

    fn finish(&mut self) {
        self.events.push(FaxEvent::Done { lines: self.rows });
        self.state = FaxState::Idle;
        let keep = (2.0 * FM_RATE) as usize;
        if self.lum.len() > keep {
            let cut = self.lum.len() - keep;
            self.lum.drain(..cut);
            self.lum_t0 += cut as u64;
        }
    }

    /// Manual slant (ppm, on top of the fitted period) and shift (fraction
    /// of a line); the image is drawn again.
    pub fn adjust(&mut self, slant_ppm: f64, shift: f64) {
        self.slant_ppm = slant_ppm;
        self.shift = shift.rem_euclid(1.0);
        if self.state == FaxState::Image {
            self.period = self.period_fit * (1.0 + slant_ppm * 1e-6);
            self.redraw();
        }
    }

    pub fn set_ioc(&mut self, ioc: u32) {
        self.ioc = ioc;
    }

    fn redraw(&mut self) {
        self.rows = 0;
        self.events.push(FaxEvent::Redraw);
        self.render_new();
    }

    fn line_start(&self, k: usize) -> f64 {
        self.start + (k as f64 + self.shift) * self.period
    }

    fn render_new(&mut self) {
        if !self.phased
            && (self.n as f64)
                > self.start + (PHASING_LINES as f64 + 2.0) * self.period + 1.0 * FM_RATE
        {
            self.phased = true;
            if self.phase() {
                self.redraw();
                return;
            }
        }
        let w = self.width();
        let first = self.rows;
        let mut data = Vec::new();
        while self.rows < MAX_LINES && self.line_start(self.rows + 1) + 2.0 < self.n as f64 {
            let s = self.line_start(self.rows) - self.lum_t0 as f64;
            for i in 0..w {
                let t = s + (i as f64 + 0.5) / w as f64 * self.period;
                let k = t.floor();
                let v = if k < 0.0 || k as usize + 1 >= self.lum.len() {
                    1.0
                } else {
                    let f = (t - k) as f32;
                    let k = k as usize;
                    self.lum[k] + f * (self.lum[k + 1] - self.lum[k])
                };
                data.push((v * 255.0).round() as u8);
            }
            self.rows += 1;
        }
        if !data.is_empty() {
            self.events.push(FaxEvent::Rows { first, data });
        }
        if self.rows >= MAX_LINES {
            self.finish();
        }
    }

    /// Line start and period from the phasing pulses; false when they
    /// aren't clear (the nominal timing stays).
    fn phase(&mut self) -> bool {
        const BINS: usize = 1000;
        let p = self.period;
        let s0 = self.start + 1.0 * FM_RATE - self.lum_t0 as f64;
        let mut lines: Vec<Vec<f32>> = Vec::new();
        for j in 0..PHASING_LINES {
            let mut b = vec![0f32; BINS];
            for (i, v) in b.iter_mut().enumerate() {
                let (a, z) = (
                    s0 + j as f64 * p + i as f64 * p / BINS as f64,
                    s0 + j as f64 * p + (i + 1) as f64 * p / BINS as f64,
                );
                let (a, z) = (
                    a.max(0.0) as usize,
                    (z.max(0.0) as usize).min(self.lum.len()),
                );
                if z > a {
                    *v = self.lum[a..z].iter().sum::<f32>() / (z - a) as f32;
                }
            }
            lines.push(b);
        }
        let all: Vec<f32> = lines.iter().flatten().copied().collect();
        let med = web888_cw::dsp::median(&all);
        // the pulse is whichever way the background is not
        let dark = med > 0.5;
        let pw = BINS / 20;
        let mut pos = Vec::new();
        let mut contrast = Vec::new();
        for l in &lines {
            let x: Vec<f32> = l
                .iter()
                .map(|&v| if dark { med - v } else { v - med })
                .collect();
            // circular moving sum over the pulse width
            let mut s: f32 = x[..pw].iter().sum();
            let mut best = (s, 0usize);
            for i in 1..BINS {
                s += x[(i + pw - 1) % BINS] - x[i - 1];
                if s > best.0 {
                    best = (s, i);
                }
            }
            // sub-bin: a parabola through the moving sums around the peak
            let ms = |i: usize| -> f32 { (0..pw).map(|k| x[(i + k) % BINS]).sum() };
            let (a, b, c) = (
                ms((best.1 + BINS - 1) % BINS),
                best.0,
                ms((best.1 + 1) % BINS),
            );
            let den = a - 2.0 * b + c;
            let d = if den < 0.0 {
                (0.5 * (a - c) / den) as f64
            } else {
                0.0
            };
            pos.push(best.1 as f64 + d + pw as f64 / 2.0);
            contrast.push(best.0 / pw as f32);
        }
        if web888_cw::dsp::median(&contrast) < 0.3 {
            return false;
        }
        // unwrap around the line, fit pos = a + b j
        for j in 1..pos.len() {
            while pos[j] - pos[j - 1] > BINS as f64 / 2.0 {
                pos[j] -= BINS as f64;
            }
            while pos[j] - pos[j - 1] < -(BINS as f64) / 2.0 {
                pos[j] += BINS as f64;
            }
        }
        // least squares pos = a + b j, lines far off the line dropped once
        let line_fit = |pts: &[(f64, f64)]| -> (f64, f64) {
            let n = pts.len() as f64;
            let mj = pts.iter().map(|p| p.0).sum::<f64>() / n;
            let mp = pts.iter().map(|p| p.1).sum::<f64>() / n;
            let sxy: f64 = pts.iter().map(|p| (p.0 - mj) * (p.1 - mp)).sum();
            let sxx: f64 = pts.iter().map(|p| (p.0 - mj).powi(2)).sum();
            let b = if sxx > 0.0 { sxy / sxx } else { 0.0 };
            (mp - b * mj, b)
        };
        let mut pts: Vec<(f64, f64)> = pos
            .iter()
            .enumerate()
            .map(|(j, &y)| (j as f64, y))
            .collect();
        let (mut a, mut b) = line_fit(&pts);
        let mut res: Vec<f32> = pts
            .iter()
            .map(|p| (p.1 - a - b * p.0).abs() as f32)
            .collect();
        let tol = (3.0 * web888_cw::dsp::median(&res) as f64).max(2.0);
        pts.retain(|p| (p.1 - a - b * p.0).abs() <= tol);
        if pts.len() < PHASING_LINES / 2 {
            return false;
        }
        (a, b) = line_fit(&pts);
        res = pts
            .iter()
            .map(|p| (p.1 - a - b * p.0).abs() as f32)
            .collect();
        let rms = (res.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / res.len() as f64).sqrt();
        if rms > 16.0 {
            return false;
        }
        self.period_fit = p / (1.0 + self.slant_ppm * 1e-6) * (1.0 + b / BINS as f64);
        self.period = self.period_fit * (1.0 + self.slant_ppm * 1e-6);
        // the pulse centred 2.5 % into line 0 of the phasing window
        let mut start = s0 + self.lum_t0 as f64 + (a / BINS as f64) * p - 0.025 * self.period;
        let img = self.lum_t0 as f64;
        while start - self.period >= img {
            start -= self.period;
        }
        while start < img {
            start += self.period;
        }
        self.start = start;
        true
    }
}
