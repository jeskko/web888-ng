// SPDX-License-Identifier: GPL-3.0-or-later
//! SSTV: the VIS code picks the mode (Martin 1/2, Scottie 1/2/DX, Robot
//! 36/72, PD 50/90/120/160/180/240); the image's lines are timed from its
//! sync pulses (1200 Hz), fitted as start + k × period, so a clock a few
//! hundred ppm off doesn't slant it. Pixels: 1500 Hz black to 2300 Hz
//! white; Martin and Scottie send G, B, R; Robot and PD luminance and
//! colour differences (Robot 36 one difference per line, alternating).

use crate::fm::FM_RATE;

/// How a mode's channels make colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    /// Channels G, B, R.
    Gbr,
    /// Y and one colour difference per line (Robot 36: R−Y on even lines,
    /// B−Y on odd, told by the separator's frequency).
    Yc420,
    /// Y, R−Y, B−Y.
    Yuv,
    /// Two lines per sync: Y even, R−Y, B−Y, Y odd.
    Pd,
}

/// One SSTV mode's timing (ms).
#[derive(Clone, Debug, PartialEq)]
pub struct Mode {
    pub name: &'static str,
    pub vis: u8,
    pub width: usize,
    pub height: usize,
    /// Sync to sync.
    pub period: f64,
    pub sync: f64,
    /// From the image start (after VIS) to the first line's sync.
    pub first_sync: f64,
    /// (start relative to the sync's start, duration, channel).
    pub scans: &'static [(f64, f64, u8)],
    pub color: Color,
}

impl Mode {
    pub fn lines_per_sync(&self) -> usize {
        if self.color == Color::Pd { 2 } else { 1 }
    }

    pub fn syncs(&self) -> usize {
        self.height / self.lines_per_sync()
    }
}

const M1: f64 = 146.432;
const M2: f64 = 73.216;
const S1: f64 = 138.24;
const S2: f64 = 88.064;
const SDX: f64 = 345.6;

macro_rules! pd {
    ($name:expr, $vis:expr, $w:expr, $h:expr, $t:expr) => {
        Mode {
            name: $name,
            vis: $vis,
            width: $w,
            height: $h,
            period: 22.08 + 4.0 * $t,
            sync: 20.0,
            first_sync: 0.0,
            scans: &[
                (22.08, $t, 0),
                (22.08 + $t, $t, 1),
                (22.08 + 2.0 * $t, $t, 2),
                (22.08 + 3.0 * $t, $t, 3),
            ],
            color: Color::Pd,
        }
    };
}

/// The modes decoded.
pub static MODES: &[Mode] = &[
    Mode {
        name: "Martin 1",
        vis: 44,
        width: 320,
        height: 256,
        period: 446.446,
        sync: 4.862,
        first_sync: 0.0,
        scans: &[
            (5.434, M1, 0),
            (5.434 + M1 + 0.572, M1, 1),
            (5.434 + 2.0 * (M1 + 0.572), M1, 2),
        ],
        color: Color::Gbr,
    },
    Mode {
        name: "Martin 2",
        vis: 40,
        width: 320,
        height: 256,
        period: 226.798,
        sync: 4.862,
        first_sync: 0.0,
        scans: &[
            (5.434, M2, 0),
            (5.434 + M2 + 0.572, M2, 1),
            (5.434 + 2.0 * (M2 + 0.572), M2, 2),
        ],
        color: Color::Gbr,
    },
    Mode {
        name: "Scottie 1",
        vis: 60,
        width: 320,
        height: 256,
        period: 428.22,
        sync: 9.0,
        first_sync: 12.0 + 2.0 * S1,
        scans: &[(-(2.0 * S1 + 1.5), S1, 0), (-S1, S1, 1), (10.5, S1, 2)],
        color: Color::Gbr,
    },
    Mode {
        name: "Scottie 2",
        vis: 56,
        width: 320,
        height: 256,
        period: 277.692,
        sync: 9.0,
        first_sync: 12.0 + 2.0 * S2,
        scans: &[(-(2.0 * S2 + 1.5), S2, 0), (-S2, S2, 1), (10.5, S2, 2)],
        color: Color::Gbr,
    },
    Mode {
        name: "Scottie DX",
        vis: 76,
        width: 320,
        height: 256,
        period: 1050.3,
        sync: 9.0,
        first_sync: 12.0 + 2.0 * SDX,
        scans: &[(-(2.0 * SDX + 1.5), SDX, 0), (-SDX, SDX, 1), (10.5, SDX, 2)],
        color: Color::Gbr,
    },
    Mode {
        name: "Robot 36",
        vis: 8,
        width: 320,
        height: 240,
        period: 150.0,
        sync: 9.0,
        first_sync: 0.0,
        scans: &[(12.0, 88.0, 0), (106.0, 44.0, 1)],
        color: Color::Yc420,
    },
    Mode {
        name: "Robot 72",
        vis: 12,
        width: 320,
        height: 240,
        period: 300.0,
        sync: 9.0,
        first_sync: 0.0,
        scans: &[(12.0, 138.0, 0), (156.0, 69.0, 1), (231.0, 69.0, 2)],
        color: Color::Yuv,
    },
    pd!("PD 50", 93, 320, 256, 91.52),
    pd!("PD 90", 99, 320, 256, 170.24),
    pd!("PD 120", 95, 640, 496, 121.6),
    pd!("PD 160", 98, 512, 400, 195.584),
    pd!("PD 180", 96, 640, 496, 183.04),
    pd!("PD 240", 97, 640, 496, 244.48),
];

pub fn mode_by_vis(vis: u8) -> Option<&'static Mode> {
    MODES.iter().find(|m| m.vis == vis)
}

/// What the decoder reports.
#[derive(Clone, Debug, PartialEq)]
pub enum SstvEvent {
    /// A new image in this mode.
    Image {
        mode: &'static str,
        width: usize,
        height: usize,
    },
    /// RGB rows `first..` (3 × width bytes each).
    Rows {
        first: usize,
        data: Vec<u8>,
    },
    /// Everything is drawn again: rows from 0 follow.
    Redraw,
    Done {
        lines: usize,
    },
}

const SYNC_HZ: f32 = 1350.0;

fn ms(v: f64) -> f64 {
    v * FM_RATE / 1000.0
}

fn val(f: f32) -> f32 {
    ((f - 1500.0) / 800.0 * 255.0).clamp(0.0, 255.0)
}

struct Image {
    mode: &'static Mode,
    /// Freq since the image start (FM samples).
    t0: u64,
    freq: Vec<f32>,
    /// Sync starts found (absolute FM samples), with their index.
    syncs: Vec<(usize, f64)>,
    /// Fit: sync k starts at a + b k.
    a: f64,
    b: f64,
    /// Where the sync search goes on (index into freq).
    scan: usize,
    /// Rows drawn, syncs drawn.
    rows: usize,
    drawn: usize,
    redraws: usize,
    /// Robot 36: each line's chroma, and whether it carried R−Y.
    chroma: Vec<(Vec<f32>, bool)>,
    luma: Vec<Vec<f32>>,
}

pub struct Sstv {
    /// FM samples in.
    n: u64,
    /// Recent freq for VIS (since `hist_t0`).
    hist: Vec<f32>,
    hist_t0: u64,
    vis_scan: u64,
    img: Option<Image>,
    events: Vec<SstvEvent>,
}

impl Default for Sstv {
    fn default() -> Self {
        Self::new()
    }
}

impl Sstv {
    pub fn new() -> Self {
        Sstv {
            n: 0,
            hist: Vec::new(),
            hist_t0: 0,
            vis_scan: 0,
            img: None,
            events: Vec::new(),
        }
    }

    pub fn poll(&mut self) -> Vec<SstvEvent> {
        std::mem::take(&mut self.events)
    }

    /// The mode and rows of the image being received.
    pub fn status(&self) -> Option<(&'static str, usize, f64)> {
        self.img
            .as_ref()
            .map(|i| (i.mode.name, i.rows, (i.b / ms(i.mode.period) - 1.0) * 1e6))
    }

    pub fn push(&mut self, freq: &[f32]) {
        self.hist.extend_from_slice(freq);
        self.n += freq.len() as u64;
        if let Some(img) = &mut self.img {
            img.freq.extend_from_slice(freq);
        }
        if self.img.is_some() {
            self.track();
        }
        if self.img.is_none() {
            self.find_vis();
        }
        let keep = ms(1500.0) as usize;
        if self.hist.len() > 2 * keep {
            let cut = self.hist.len() - keep;
            self.hist.drain(..cut);
            self.hist_t0 += cut as u64;
        }
    }

    /// Mean freq over [a, b) ms from absolute sample e.
    fn mean_at(&self, e: f64, a: f64, b: f64) -> Option<f32> {
        let i0 = (e + ms(a)).round() as i64 - self.hist_t0 as i64;
        let i1 = (e + ms(b)).round() as i64 - self.hist_t0 as i64;
        if i0 < 0 || i1 as usize > self.hist.len() || i1 <= i0 {
            return None;
        }
        let s = &self.hist[i0 as usize..i1 as usize];
        Some(s.iter().sum::<f32>() / s.len() as f32)
    }

    fn find_vis(&mut self) {
        // a start bit's leading edge: leader (1900) falling to 1200
        let start = self.vis_scan.max(self.hist_t0 + ms(260.0) as u64);
        let end = self.n.saturating_sub(ms(310.0) as u64);
        let mut e = start;
        while e < end {
            let i = (e - self.hist_t0) as usize;
            if i >= 1
                && self.hist[i - 1] > SYNC_HZ
                && self.hist[i] <= SYNC_HZ
                && self.try_vis(e as f64)
            {
                return;
            }
            e += 1;
        }
        self.vis_scan = e;
    }

    fn try_vis(&mut self, e: f64) -> bool {
        let near = |v: Option<f32>, f: f32, tol: f32| v.is_some_and(|v| (v - f).abs() < tol);
        if !near(self.mean_at(e, -250.0, -20.0), 1900.0, 80.0)
            || !near(self.mean_at(e, 5.0, 25.0), 1200.0, 80.0)
        {
            return false;
        }
        let mut code = 0u8;
        let mut ones = 0;
        for i in 0..8 {
            let Some(f) =
                self.mean_at(e, 30.0 * (i + 1) as f64 + 5.0, 30.0 * (i + 1) as f64 + 25.0)
            else {
                return false;
            };
            let bit = if (f - 1100.0).abs() < 80.0 {
                1
            } else if (f - 1300.0).abs() < 80.0 {
                0
            } else {
                return false;
            };
            ones += bit;
            if i < 7 {
                code |= (bit as u8) << i;
            }
        }
        if ones % 2 != 0 || !near(self.mean_at(e, 275.0, 295.0), 1200.0, 80.0) {
            return false;
        }
        let Some(mode) = mode_by_vis(code) else {
            return false;
        };
        let t_img = e + ms(300.0);
        let t0 = t_img.round() as u64;
        let from = (t0 - self.hist_t0) as usize;
        let freq = self.hist[from.min(self.hist.len())..].to_vec();
        self.events.push(SstvEvent::Image {
            mode: mode.name,
            width: mode.width,
            height: mode.height,
        });
        self.img = Some(Image {
            mode,
            t0,
            freq,
            syncs: Vec::new(),
            a: t_img + ms(mode.first_sync),
            b: ms(mode.period),
            scan: 0,
            rows: 0,
            drawn: 0,
            redraws: 0,
            chroma: Vec::new(),
            luma: Vec::new(),
        });
        self.vis_scan = self.n;
        self.track();
        true
    }

    /// Find syncs, refit the timing, draw what's complete.
    fn track(&mut self) {
        let Some(img) = &mut self.img else {
            return;
        };
        let m = img.mode;
        let min_run = (0.5 * ms(m.sync)) as usize;
        let f = std::mem::take(&mut img.freq);
        // smoothed over 1 ms (FM clicks in noise), and the run's mean
        // below the threshold, not every sample
        let sw = ms(1.0).max(1.0) as usize;
        let sm = |j: usize| f[j..j + sw].iter().sum::<f32>() / sw as f32;
        let mut i = img.scan.max(1);
        while i + min_run + sw < f.len() {
            if sm(i) <= SYNC_HZ
                && sm(i - 1) > SYNC_HZ
                && f[i..i + min_run].iter().sum::<f32>() / (min_run as f32) <= SYNC_HZ
            {
                let (p, q) = (sm(i - 1), sm(i));
                let edge = i as f64 - 1.0
                    + (p - SYNC_HZ) as f64 / (p - q) as f64
                    + 0.5 * (sw as f64 - 1.0);
                let t = img.t0 as f64 + edge;
                let k = ((t - img.a) / img.b).round();
                if k >= 0.0
                    && (k as usize) < m.syncs()
                    && (t - img.a - k * img.b).abs() < 0.05 * img.b
                {
                    img.syncs.retain(|s| s.0 != k as usize);
                    img.syncs.push((k as usize, t));
                    fit(img);
                }
                i += min_run;
                continue;
            }
            i += 1;
        }
        img.scan = i;
        img.freq = f;
        // redraw when the fit has settled more (8, 32 syncs)
        let n = img.syncs.len();
        if (img.redraws == 0 && n >= 8) || (img.redraws == 1 && n >= 32) {
            img.redraws += 1;
            img.drawn = 0;
            img.rows = 0;
            img.luma.clear();
            img.chroma.clear();
            self.events.push(SstvEvent::Redraw);
        }
        self.draw();
    }

    fn draw(&mut self) {
        let Some(img) = &mut self.img else {
            return;
        };
        let m = img.mode;
        let end = img.t0 as f64 + img.freq.len() as f64;
        let last = m.scans.iter().map(|s| s.0 + s.1).fold(0.0, f64::max);
        let first = img.rows;
        let mut data = Vec::new();
        while img.drawn < m.syncs() && img.a + img.b * img.drawn as f64 + ms(last) + 2.0 < end {
            let k = img.drawn;
            let s0 = img.a + img.b * k as f64 - img.t0 as f64;
            let scale = img.b / ms(m.period);
            let chan = |c: u8| -> Vec<f32> {
                let (off, dur, _) = *m.scans.iter().find(|s| s.2 == c).expect("channel");
                // each pixel: the mean frequency over its time (noise
                // averages out), at least the value at its centre
                let px = ms(dur / m.width as f64) * scale;
                (0..m.width)
                    .map(|x| {
                        let t = s0 + ms(off + (x as f64 + 0.5) / m.width as f64 * dur) * scale;
                        let i = t.floor();
                        if i < 0.0 || i as usize + 1 >= img.freq.len() {
                            return 0.0;
                        }
                        let fr = (t - i) as f32;
                        let iu = i as usize;
                        let centre = img.freq[iu] + fr * (img.freq[iu + 1] - img.freq[iu]);
                        let (a, b) = (
                            (t - 0.5 * px).ceil().max(0.0) as usize,
                            ((t + 0.5 * px).floor() as usize).min(img.freq.len() - 1),
                        );
                        if b > a {
                            let s: f32 = img.freq[a..=b].iter().sum();
                            val((s + centre) / (b - a + 2) as f32)
                        } else {
                            val(centre)
                        }
                    })
                    .collect()
            };
            match m.color {
                Color::Gbr => {
                    let (g, b, r) = (chan(0), chan(1), chan(2));
                    for x in 0..m.width {
                        data.extend([r[x] as u8, g[x] as u8, b[x] as u8]);
                    }
                    img.rows += 1;
                }
                Color::Yuv => {
                    let (y, v, u) = (chan(0), chan(1), chan(2));
                    push_yuv(&mut data, &y, &u, &v);
                    img.rows += 1;
                }
                Color::Pd => {
                    let (y0, v, u, y1) = (chan(0), chan(1), chan(2), chan(3));
                    push_yuv(&mut data, &y0, &u, &v);
                    push_yuv(&mut data, &y1, &u, &v);
                    img.rows += 2;
                }
                Color::Yc420 => {
                    // the separator (100..104.5 ms): 1500 Hz R−Y, 2300 Hz B−Y
                    let sep = {
                        let (a, b) = (
                            (s0 + ms(100.5) * scale) as usize,
                            (s0 + ms(104.0) * scale) as usize,
                        );
                        let s = &img.freq[a.min(img.freq.len())..b.min(img.freq.len())];
                        if s.is_empty() {
                            1900.0
                        } else {
                            s.iter().sum::<f32>() / s.len() as f32
                        }
                    };
                    let is_v = if (sep - 1900.0).abs() < 200.0 {
                        k % 2 == 0
                    } else {
                        sep < 1900.0
                    };
                    img.luma.push(chan(0));
                    img.chroma.push((chan(1), is_v));
                    // rows go out in pairs, each half of the pair's chroma
                    if img.luma.len() == 2 {
                        let (c0, c1) = (&img.chroma[0], &img.chroma[1]);
                        let (v, u) = if c0.1 || !c1.1 {
                            (&c0.0, &c1.0)
                        } else {
                            (&c1.0, &c0.0)
                        };
                        push_yuv(&mut data, &img.luma[0], u, v);
                        push_yuv(&mut data, &img.luma[1], u, v);
                        img.rows += 2;
                        img.luma.clear();
                        img.chroma.clear();
                    }
                }
            }
            img.drawn += 1;
        }
        if !data.is_empty() {
            self.events.push(SstvEvent::Rows { first, data });
        }
        // done: all lines, or no sync for 20 lines (the sender stopped)
        let lost = img.syncs.last().map(|s| s.1).unwrap_or(img.a);
        if img.drawn >= m.syncs() || end - lost > 20.0 * img.b + ms(m.first_sync) {
            let lines = img.rows;
            self.events.push(SstvEvent::Done { lines });
            let next = img.t0 as f64 + img.freq.len() as f64;
            self.img = None;
            self.vis_scan = next as u64;
        }
    }

    /// End the image now.
    pub fn stop_now(&mut self) {
        if let Some(img) = self.img.take() {
            self.events.push(SstvEvent::Done { lines: img.rows });
            self.vis_scan = self.n;
        }
    }
}

fn push_yuv(out: &mut Vec<u8>, y: &[f32], u: &[f32], v: &[f32]) {
    for x in 0..y.len() {
        let (yy, uu, vv) = (y[x], u[x] - 128.0, v[x] - 128.0);
        let r = yy + 1.402 * vv;
        let g = yy - 0.344_136 * uu - 0.714_136 * vv;
        let b = yy + 1.772 * uu;
        out.extend([
            r.clamp(0.0, 255.0) as u8,
            g.clamp(0.0, 255.0) as u8,
            b.clamp(0.0, 255.0) as u8,
        ]);
    }
}

/// Least squares a + b k over the syncs found, outliers (> 1.5 ms) out.
fn fit(img: &mut Image) {
    if img.syncs.len() < 2 {
        if let Some(&(k, t)) = img.syncs.first() {
            img.a = t - img.b * k as f64;
        }
        return;
    }
    let mut pts: Vec<(f64, f64)> = img.syncs.iter().map(|&(k, t)| (k as f64, t)).collect();
    for _ in 0..2 {
        let n = pts.len() as f64;
        let mk = pts.iter().map(|p| p.0).sum::<f64>() / n;
        let mt = pts.iter().map(|p| p.1).sum::<f64>() / n;
        let sxx: f64 = pts.iter().map(|p| (p.0 - mk).powi(2)).sum();
        if sxx == 0.0 {
            return;
        }
        let b = pts.iter().map(|p| (p.0 - mk) * (p.1 - mt)).sum::<f64>() / sxx;
        let a = mt - b * mk;
        // a period within 1 % of nominal (a slant beyond that is no clock error)
        let nominal = ms(img.mode.period);
        if (b / nominal - 1.0).abs() < 0.01 {
            img.a = a;
            img.b = b;
        }
        let tol = ms(1.5);
        let kept: Vec<(f64, f64)> = pts
            .iter()
            .copied()
            .filter(|p| (p.1 - img.a - img.b * p.0).abs() < tol)
            .collect();
        if kept.len() < 2 || kept.len() == pts.len() {
            break;
        }
        pts = kept;
    }
}

/// A piece of a synthesised signal: start (ms), duration (ms), frequency
/// at a fraction 0..1 of it.
type Seg<'a> = (f64, f64, Box<dyn Fn(f64) -> f32 + 'a>);

/// Synthesis for tests: the frequency sequence (one value per sample at
/// `rate`) of an image in `mode`, VIS first. `rgb(x, y)` gives a pixel.
pub fn encode(mode: &Mode, rgb: &dyn Fn(usize, usize) -> [u8; 3], rate: f64) -> Vec<f32> {
    // piecewise: (start ms, duration ms, freq at fraction 0..1)
    let mut seg: Vec<Seg<'_>> = Vec::new();
    let mut t = 0.0;
    let tone = |seg: &mut Vec<Seg<'_>>, t: &mut f64, d: f64, f: f32| {
        seg.push((*t, d, Box::new(move |_| f)));
        *t += d;
    };
    tone(&mut seg, &mut t, 300.0, 1900.0);
    tone(&mut seg, &mut t, 10.0, 1200.0);
    tone(&mut seg, &mut t, 300.0, 1900.0);
    tone(&mut seg, &mut t, 30.0, 1200.0);
    let mut ones = 0;
    for i in 0..7 {
        let b = (mode.vis >> i) & 1;
        ones += b;
        tone(&mut seg, &mut t, 30.0, if b == 1 { 1100.0 } else { 1300.0 });
    }
    tone(
        &mut seg,
        &mut t,
        30.0,
        if ones % 2 == 1 { 1100.0 } else { 1300.0 },
    );
    tone(&mut seg, &mut t, 30.0, 1200.0);
    let t_img = t;
    if mode.first_sync > 0.0 {
        tone(&mut seg, &mut t, mode.sync, 1200.0); // Scottie's starting sync
    }
    let to_f = |v: f32| 1500.0 + v.clamp(0.0, 255.0) / 255.0 * 800.0;
    let yuv = |x: usize, y: usize| {
        let [r, g, b] = rgb(x, y).map(|v| v as f32);
        let yy = 0.299 * r + 0.587 * g + 0.114 * b;
        (yy, 128.0 + (b - yy) / 1.772, 128.0 + (r - yy) / 1.402)
    };
    // fill each sync period: porch 1500, sync 1200, scans
    for k in 0..mode.syncs() {
        let s = t_img + mode.first_sync + k as f64 * mode.period;
        seg.push((s, mode.sync, Box::new(|_| 1200.0)));
        if mode.color == Color::Yc420 {
            let f = if k % 2 == 0 { 1500.0 } else { 2300.0 };
            seg.push((s + 100.0, 4.5, Box::new(move |_| f)));
        }
        for &(off, dur, c) in mode.scans {
            let w = mode.width;
            let f: Box<dyn Fn(f64) -> f32> = Box::new(move |fr: f64| {
                let x = ((fr * w as f64) as usize).min(w - 1);
                let v = match (mode.color, c) {
                    (Color::Gbr, 0) => rgb(x, k)[1] as f32,
                    (Color::Gbr, 1) => rgb(x, k)[2] as f32,
                    (Color::Gbr, _) => rgb(x, k)[0] as f32,
                    (Color::Yuv, 0) | (Color::Yc420, 0) => yuv(x, k).0,
                    (Color::Yuv, 1) => yuv(x, k).2,
                    (Color::Yuv, _) => yuv(x, k).1,
                    (Color::Yc420, _) => {
                        let pair = k & !1;
                        let (_, u, v) = yuv(x, pair);
                        if k % 2 == 0 { v } else { u }
                    }
                    (Color::Pd, 0) => yuv(x, 2 * k).0,
                    (Color::Pd, 3) => yuv(x, 2 * k + 1).0,
                    (Color::Pd, 1) => 0.5 * (yuv(x, 2 * k).2 + yuv(x, 2 * k + 1).2),
                    (Color::Pd, _) => 0.5 * (yuv(x, 2 * k).1 + yuv(x, 2 * k + 1).1),
                };
                to_f(v)
            });
            seg.push((s + off, dur, f));
        }
    }
    let end = t_img + mode.first_sync + mode.syncs() as f64 * mode.period + 50.0;
    let n = (end / 1000.0 * rate) as usize;
    seg.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out = Vec::with_capacity(n);
    let mut j = 0;
    for i in 0..n {
        let tm = i as f64 * 1000.0 / rate;
        while j + 1 < seg.len() && seg[j + 1].0 <= tm {
            j += 1;
        }
        let (s, d, f) = &seg[j];
        out.push(if tm < s + d { f((tm - s) / d) } else { 1500.0 });
    }
    out
}
