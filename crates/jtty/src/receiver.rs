// SPDX-License-Identifier: GPL-3.0-or-later
//! The receiver (jtty-cuf `receiver.cpp`), as a stream: 2.36 s analysis
//! windows stepped by a quarter frame (0.472 s); per window a coarse sync
//! map over frequency channels, refinement and a sync-quality gate, the
//! tiered list decoder, grammar and plausibility checks, duplicate tests,
//! subtraction of each decoded frame with look-back windows re-searched,
//! an expected-frame retry, and message assembly. Messages come out as
//! they complete. Labels such as "D7" are jtty-cuf's design-note sections.
//!
//! Where jtty-cuf (and this port) differs from the WSJT-X decoder: the
//! search covers 200-3000 Hz instead of about 1200-1800 Hz, frames cut at
//! the start of the input are searched, and a decoded word must pass a
//! tone-energy plausibility test.

use crate::assembly::FRAME_PERIOD_S;
use crate::assembly::{
    Assembler, FrameEntry, MAX_OPEN_MESSAGES, MergeResult, OpenMessage, STAMP_DF_HZ, STAMP_DT_S,
    STAND_IN, Verdict, WindowDuplicates, Work, publication_text,
};
use crate::codec::decode_frame_payload;
use crate::fec::{DATA_SYMBOLS, SoftObservations, decode_tiers};
use crate::gfsk::gfsk_baseband;
use crate::protocol::{SAMPLES_PER_FRAME, SYNC_TONES, ToneFrame, encode_payload, remainder};
use num_complex::{Complex32, Complex64};
use rustfft::{Fft, FftPlanner};
use std::sync::{Arc, OnceLock};

const TWO_PI: f64 = 2.0 * std::f64::consts::PI;
const SYNC_SYMBOLS: usize = 13;
const FS: f64 = 12_000.0;
/// Analysis rate.
const FD: f64 = 6000.0;
/// Samples per symbol at FD.
const N_SYM: usize = 192;
const FRAME_12K: i64 = SAMPLES_PER_FRAME as i64; // 22,656
const STEP_12K: i64 = FRAME_12K / 4; // 5,664
const WINDOW_12K: i64 = FRAME_12K + STEP_12K; // 28,320
const WINDOW_FD: i64 = WINDOW_12K / 2; // 14,160
const ANALYTIC_SIZE: usize = 32768;
const BASEBAND_SIZE: usize = ANALYTIC_SIZE / 2;
const MAP_SIZE: usize = 8192;
const FBIN: f64 = FD / MAP_SIZE as f64; // 0.732 Hz
const MAP_STEPS: usize = 237;
const MAP_STEP_FD: i64 = 12;
const SYNC_LENGTH_FD: usize = SYNC_SYMBOLS * N_SYM; // 2,496
const FRAME_FD: usize = 59 * N_SYM; // 11,328
/// 0-3400 Hz kept (WSJT-X: 3000), so a frame with tone 0 at 3000 Hz fits.
const ANALYTIC_PASSBAND_HZ: f64 = 3400.0;
/// The window grid starts this far before the first input sample; frames
/// starting up to SEARCH_REACH_12K before it are searched; TAIL_12K zeros
/// end a finished stream.
const LEAD_12K: i64 = 480 + STEP_12K;
const SEARCH_REACH_12K: i64 = 3250;
const TAIL_12K: i64 = STEP_12K + 2 * N_SYM as i64;
// D6: exclusion half-sizes
const EXCLUDE_BINS_BELOW: i32 = 13;
const EXCLUDE_BINS_ABOVE: i32 = 15;
const EXCLUDE_STEPS_BELOW: i32 = 7;
const EXCLUDE_STEPS_ABOVE: i32 = 9;
const VICINITY_BINS: i32 = 14;
const MAX_RECORDED_SUBTRACTIONS: usize = 16;
// D8: gates
const RX_MIN_HITS: i32 = 7;
const RX_MIN_RHO_DB: f64 = 4.6;
const FIXED_MIN_HITS: i32 = 9;
const FIXED_MIN_RHO_DB: f64 = 5.0;
/// The plausibility test: energy at the decoded tones over the mean of the
/// other three (a sent frame: about 5.8 at -12 dB, 2.8 at -14 dB; a chance
/// CRC pass on noise mostly 1.4-3.0); the extension channels, where most
/// chance passes land, need more.
const MIN_DECODED_TONE_RATIO: f64 = 2.0;
const MIN_DECODED_TONE_RATIO_EXTENSION: f64 = 3.0;
const RETRY_TIMING_S: f64 = 0.1;
const RX_FREQUENCY_HZ: f64 = 1500.0;
const RX_TOLERANCE_HZ: f64 = 20.0;
const MAX_WINDOW_CANDIDATES: usize = 100;
const MAX_LOOKBACK_WINDOWS: usize = 48;
/// Published messages are rounded to a frequency in this interval.
pub const SEARCH_LOW_HZ: i32 = 200;
pub const SEARCH_HIGH_HZ: i32 = 3000;
/// `receive_messages`' bounds (jtty-cuf's): 120 s of input, 2,048 window
/// evaluations, 4,096 decoded frames, 256 messages.
pub const MAX_INPUT_SAMPLES: usize = 1_440_000;
const MAX_WINDOW_EVALUATIONS: usize = 2048;
const MAX_DECODED_FRAMES: usize = 4096;
const MAX_RESULTS: usize = 256;

/// A completed message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub text: String,
    /// Audio frequency of tone 0, Hz (rounded).
    pub frequency_hz: i32,
    /// Start of its first decoded frame, in samples (12 kHz) from the first
    /// sample pushed; negative if it began before.
    pub start_sample: i64,
    pub frames: usize,
}

struct Channel {
    b_lo: i32,
    b_hi: i32,
    center_hz: f64,
    half_width_hz: f64,
    rx: bool,
    picks: u32,
    /// Outside the reference's 1200-1800 Hz: refined and gated like Rx.
    extension: bool,
}

fn bin_floor(hz: f64) -> i32 {
    (hz / FBIN) as i32
}

/// D4: channel Rx (1500 +- 20 Hz), the fixed channels 1200-1500 and
/// 1500-1800 Hz, then extension channels tiling 200-3000 Hz (the outer
/// edges 5 Hz wider).
fn make_channels() -> Vec<Channel> {
    let (rx_lo, rx_hi) = (
        RX_FREQUENCY_HZ - RX_TOLERANCE_HZ,
        RX_FREQUENCY_HZ + RX_TOLERANCE_HZ,
    );
    let rx_picks = (RX_TOLERANCE_HZ / (14.0 * FBIN)).round().clamp(2.0, 8.0) as u32;
    let mut c = vec![Channel {
        b_lo: bin_floor(rx_lo).max(3),
        b_hi: bin_floor(rx_hi),
        center_hz: RX_FREQUENCY_HZ,
        half_width_hz: RX_TOLERANCE_HZ,
        rx: true,
        picks: rx_picks,
        extension: false,
    }];
    let fixed = [
        (1200.0, 1500.0),
        (1500.0, 1800.0),
        (195.0, 300.0),
        (300.0, 600.0),
        (600.0, 900.0),
        (900.0, 1200.0),
        (1800.0, 2100.0),
        (2100.0, 2400.0),
        (2400.0, 2700.0),
        (2700.0, 3005.0),
    ];
    for (lo, hi) in fixed {
        c.push(Channel {
            b_lo: bin_floor(lo).max(3),
            b_hi: bin_floor(hi),
            center_hz: 0.5 * (lo + hi),
            half_width_hz: 0.5 * (hi - lo),
            rx: false,
            picks: 2,
            extension: !((lo, hi) == (1200.0, 1500.0) || (lo, hi) == (1500.0, 1800.0)),
        });
    }
    c
}

struct Tables {
    analytic: Arc<dyn Fft<f32>>,
    baseband: Arc<dyn Fft<f32>>,
    map: Arc<dyn Fft<f32>>,
    scratch_len: usize,
    /// conj of the sync word's baseband, r < 2496 (D5)
    sync_conj: Vec<Complex32>,
    /// exp(-j 2 pi k r / n)
    tone_conj: [[Complex32; N_SYM]; 4],
    /// sum of the 385 cos^2 smoother taps (D12.1)
    smoother_sum: f64,
    channels: Vec<Channel>,
    union_lo: i32,
    union_hi: i32,
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let mut planner = FftPlanner::new();
        let analytic = planner.plan_fft_forward(ANALYTIC_SIZE);
        let baseband = planner.plan_fft_inverse(BASEBAND_SIZE);
        let map = planner.plan_fft_forward(MAP_SIZE);
        let scratch_len = analytic
            .get_inplace_scratch_len()
            .max(baseband.get_inplace_scratch_len())
            .max(map.get_inplace_scratch_len());
        let mut sync_conj = Vec::with_capacity(SYNC_LENGTH_FD);
        let mut phase = 0.0f64;
        for &tone in &SYNC_TONES {
            for _ in 0..N_SYM {
                sync_conj.push(Complex32::new(phase.cos() as f32, -phase.sin() as f32));
                phase += TWO_PI * tone as f64 / N_SYM as f64;
            }
        }
        let mut tone_conj = [[Complex32::new(0.0, 0.0); N_SYM]; 4];
        for (k, row) in tone_conj.iter_mut().enumerate() {
            for (r, v) in row.iter_mut().enumerate() {
                let a = -TWO_PI * k as f64 * r as f64 / N_SYM as f64;
                *v = Complex32::new(a.cos() as f32, a.sin() as f32);
            }
        }
        let n = N_SYM as i64;
        let smoother_sum = (-n..=n)
            .map(|j| {
                let c = (std::f64::consts::PI * j as f64 / (2.0 * N_SYM as f64)).cos();
                c * c
            })
            .sum();
        let channels = make_channels();
        let union_lo = channels.iter().map(|c| c.b_lo).min().unwrap();
        let union_hi = channels.iter().map(|c| c.b_hi).max().unwrap();
        Tables {
            analytic,
            baseband,
            map,
            scratch_len,
            sync_conj,
            tone_conj,
            smoother_sum,
            channels,
            union_lo,
            union_hi,
        }
    })
}

#[derive(Clone)]
struct RecordedSignal {
    frequency_hz: f64,
    start_12k: f64,
    tones: ToneFrame,
}

struct WindowState {
    p: i64,
    records: usize,
    duplicates: WindowDuplicates,
    record: bool,
    recorded: Vec<RecordedSignal>,
    subtracted: bool,
    lookback: bool,
}

/// The streaming receiver: push 12 kHz mono samples, get messages as they
/// complete.
pub struct Receiver {
    t: &'static Tables,
    /// The stream from absolute sample `base` (the stream begins with
    /// LEAD_12K zeros before the first pushed sample).
    samples: Vec<f32>,
    base: i64,
    next_p: i64,
    x: Vec<Complex32>,
    scratch: Vec<Complex32>,
    fft_scratch: Vec<Complex32>,
    map_buf: Vec<Complex32>,
    map: Vec<f32>,
    map_stale: bool,
    assembler: Assembler,
    work: Work,
    out: Vec<Message>,
    /// jtty-cuf's per-reception bounds (receive_messages)
    bounded: bool,
    evaluations: usize,
    results: usize,
    failure: Option<String>,
}

impl Default for Receiver {
    fn default() -> Self {
        Self::new()
    }
}

impl Receiver {
    pub fn new() -> Self {
        let t = tables();
        Receiver {
            t,
            samples: vec![0.0; LEAD_12K as usize],
            base: 0,
            next_p: 0,
            x: vec![Complex32::new(0.0, 0.0); BASEBAND_SIZE],
            scratch: vec![Complex32::new(0.0, 0.0); ANALYTIC_SIZE],
            fft_scratch: vec![Complex32::new(0.0, 0.0); t.scratch_len],
            map_buf: vec![Complex32::new(0.0, 0.0); MAP_SIZE],
            map: vec![0.0; MAP_STEPS * (t.union_hi - t.union_lo + 1) as usize],
            map_stale: true,
            assembler: Assembler::default(),
            work: Work::default(),
            out: Vec::new(),
            bounded: false,
            evaluations: 0,
            results: 0,
            failure: None,
        }
    }

    /// Work counters so far.
    pub fn work(&self) -> &Work {
        &self.work
    }

    /// Samples (12 kHz, about +-1) in; the messages completed by them out.
    /// Non-finite samples count as zero.
    pub fn push(&mut self, pcm: &[f32]) -> Vec<Message> {
        self.samples
            .extend(pcm.iter().map(|&v| if v.is_finite() { v } else { 0.0 }));
        self.run();
        std::mem::take(&mut self.out)
    }

    /// The end of the input: the last frames are searched, open messages
    /// are cut off (unpublished, as in jtty-cuf).
    pub fn finish(&mut self) -> Vec<Message> {
        self.samples
            .extend(std::iter::repeat_n(0.0, TAIL_12K as usize));
        self.run();
        if self.failure.is_none() {
            self.assembler.finish(&mut self.work);
        }
        std::mem::take(&mut self.out)
    }

    fn run(&mut self) {
        let end = self.base + self.samples.len() as i64;
        while self.next_p + WINDOW_12K <= end && self.failure.is_none() {
            let p = self.next_p;
            self.assembler.expire(p as f64 / FS, &mut self.work);
            self.work.windows += 1;
            let recorded = self.analyze(p, None, true);
            let mut lookbacks = 0;
            for signal in &recorded {
                for k in 1..=3 {
                    let earlier = p - k * STEP_12K;
                    if earlier < 0 || lookbacks >= MAX_LOOKBACK_WINDOWS || self.failure.is_some() {
                        continue;
                    }
                    lookbacks += 1;
                    self.work.lookback_windows += 1;
                    self.analyze(earlier, Some(signal), false);
                }
            }
            self.next_p += STEP_12K;
            // keep what the next window and its look-backs read
            let keep_from = (self.next_p - 3 * STEP_12K).max(self.base);
            let drop = (keep_from - self.base) as usize;
            if drop > 0 {
                self.samples.drain(..drop);
                self.base = keep_from;
            }
        }
    }

    fn fail(&mut self, why: String) {
        if self.failure.is_none() {
            self.failure = Some(why);
        }
    }

    fn map_bins(&self) -> usize {
        (self.t.union_hi - self.t.union_lo + 1) as usize
    }

    fn map_index(&self, step: usize, bin: i32) -> usize {
        step * self.map_bins() + (bin - self.t.union_lo) as usize
    }

    /// D3: the window's analytic signal at 6 kHz.
    fn load_window(&mut self, p: i64) -> bool {
        let at = (p - self.base) as usize;
        let s = &self.samples[at..at + WINDOW_12K as usize];
        if s.iter().all(|&v| v == 0.0) {
            return false;
        }
        let scale = 2.0 / ANALYTIC_SIZE as f32;
        self.scratch.fill(Complex32::new(0.0, 0.0));
        for (d, &v) in self.scratch.iter_mut().zip(s) {
            *d = Complex32::new(v * scale, 0.0);
        }
        self.t
            .analytic
            .process_with_scratch(&mut self.scratch, &mut self.fft_scratch);
        let keep = (ANALYTIC_PASSBAND_HZ / FS * ANALYTIC_SIZE as f64) as usize;
        self.x.fill(Complex32::new(0.0, 0.0));
        self.x[..=keep].copy_from_slice(&self.scratch[..=keep]);
        self.x[0] *= 0.5;
        self.t
            .baseband
            .process_with_scratch(&mut self.x, &mut self.fft_scratch);
        self.x[WINDOW_FD as usize..].fill(Complex32::new(0.0, 0.0));
        true
    }

    /// D5: the coarse sync map over all channels.
    fn build_map(&mut self) {
        self.map_stale = false;
        let (lo, hi) = (self.t.union_lo, self.t.union_hi);
        let bins = self.map_bins();
        let mut power = vec![0f32; bins + 4];
        for s in 0..MAP_STEPS {
            let tau = s * MAP_STEP_FD as usize;
            self.map_buf.fill(Complex32::new(0.0, 0.0));
            for r in 0..SYNC_LENGTH_FD {
                self.map_buf[r] = self.x[tau + r] * self.t.sync_conj[r];
            }
            self.t
                .map
                .process_with_scratch(&mut self.map_buf, &mut self.fft_scratch);
            for b in lo - 2..=hi + 2 {
                power[(b - lo + 2) as usize] = self.map_buf[b as usize].norm_sqr();
            }
            for b in lo..=hi {
                let i = (b - lo + 2) as usize;
                self.map[s * bins + (b - lo) as usize] = power[i - 2]
                    + 2.0 * power[i - 1]
                    + 3.0 * power[i]
                    + 2.0 * power[i + 1]
                    + power[i + 2];
            }
        }
    }

    /// y[m] = x[m] exp(-j 2 pi f (m + 1) / fd) over [first, first + count);
    /// outside the buffer zero.
    fn mix(&self, frequency_hz: f64, first: i64, count: usize) -> Vec<Complex32> {
        let mut out = vec![Complex32::new(0.0, 0.0); count];
        let w = -TWO_PI * frequency_hz / FD;
        let step = Complex64::new(w.cos(), w.sin());
        let a0 = remainder(w * (first + 1) as f64, TWO_PI);
        let mut rot = Complex64::new(a0.cos(), a0.sin());
        for (i, o) in out.iter_mut().enumerate() {
            let m = first + i as i64;
            if (0..BASEBAND_SIZE as i64).contains(&m) {
                *o = self.x[m as usize] * Complex32::new(rot.re as f32, rot.im as f32);
            }
            rot *= step;
            if i & 511 == 511 {
                rot /= rot.norm();
            }
        }
        out
    }

    /// D7: refinement of a candidate; (frequency, offset in fd samples).
    fn refine(&self, f: f64, x_s: f64) -> (f64, i64) {
        let t_a = ((x_s - 0.004) * FD).round().max(0.0) as i64;
        let t_b = (WINDOW_FD - SYNC_LENGTH_FD as i64).min(((x_s + 0.004) * FD).round() as i64);
        let mut best = 0.0f64;
        let (mut best_f, mut best_t) = (0.0, 0i64);
        let mut best_z = [Complex64::new(0.0, 0.0); SYNC_SYMBOLS];
        for k in -5..=5 {
            let fh = f - 0.5 * k as f64;
            if t_a > t_b {
                break;
            }
            let y = self.mix(fh, t_a, (t_b - t_a) as usize + SYNC_LENGTH_FD);
            let mut t = t_a;
            while t <= t_b {
                let mut metric = 0.0;
                let mut z = [Complex64::new(0.0, 0.0); SYNC_SYMBOLS];
                for (i, zi) in z.iter_mut().enumerate() {
                    let base = (t - t_a) as usize + i * N_SYM;
                    let mut sum = Complex64::new(0.0, 0.0);
                    for r in 0..N_SYM {
                        sum += c64(y[base + r]) * c64(self.t.sync_conj[i * N_SYM + r]);
                    }
                    *zi = sum;
                    metric += sum.norm_sqr();
                }
                if metric > best {
                    best = metric;
                    best_f = fh;
                    best_t = t;
                    best_z = z;
                }
                t += 4;
            }
        }
        if !(best > 0.0) {
            return (0.0, 0);
        }
        // linear-phase correction
        let mut u = [0.0f64; SYNC_SYMBOLS];
        u[0] = best_z[0].arg();
        for i in 1..SYNC_SYMBOLS {
            let mut d = best_z[i].arg() - best_z[i - 1].arg();
            while d > std::f64::consts::PI {
                d -= TWO_PI;
            }
            while d < -std::f64::consts::PI {
                d += TWO_PI;
            }
            u[i] = u[i - 1] + d;
        }
        let mean = u.iter().sum::<f64>() / SYNC_SYMBOLS as f64;
        let (mut num, mut den) = (0.0, 0.0);
        for (i, &v) in u.iter().enumerate() {
            let c = i as f64 - 6.0;
            num += c * (v - mean);
            den += c * c;
        }
        let beta = num / den;
        let a = mean - 6.0 * beta;
        let residual = (u
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let e = v - a - beta * i as f64;
                e * e
            })
            .sum::<f64>()
            / SYNC_SYMBOLS as f64)
            .sqrt();
        let df = beta / (TWO_PI * 0.032);
        if residual < 1.0 && df.abs() <= 0.5 {
            best_f += df;
        }
        (best_f, best_t)
    }

    /// D8: the sync-quality gate at (f, t).
    fn gate(&self, f: f64, t: i64, channel: &Channel) -> bool {
        let y = self.mix(f, t, SYNC_LENGTH_FD);
        let mut hits = 0;
        let (mut on, mut total) = (0.0f64, 0.0f64);
        for i in 0..SYNC_SYMBOLS {
            let ti = t + (i * N_SYM) as i64;
            if ti < 0 || ti + N_SYM as i64 > WINDOW_FD {
                continue;
            }
            let mut w = [0.0f64; 4];
            for (k, wk) in w.iter_mut().enumerate() {
                let mut sum = Complex64::new(0.0, 0.0);
                for r in 0..N_SYM {
                    sum += c64(y[i * N_SYM + r]) * c64(self.t.tone_conj[k][r]);
                }
                *wk = sum.norm_sqr();
            }
            let expected = SYNC_TONES[i] as usize;
            let mut strongest = 0;
            for k in 1..4 {
                if w[k] > w[strongest] {
                    strongest = k;
                }
            }
            if strongest == expected {
                hits += 1;
            }
            on += w[expected];
            total += w.iter().sum::<f64>();
        }
        let off = (total - on) / 3.0;
        let ratio = if off > 0.0 { on / off } else { 0.0 };
        let rho = if off > 0.0 {
            if ratio > 1.259e-10 {
                10.0 * ratio.log10()
            } else {
                -99.0
            }
        } else {
            -99.9
        };
        if channel.rx || channel.extension {
            hits >= RX_MIN_HITS && rho >= RX_MIN_RHO_DB
        } else {
            hits >= FIXED_MIN_HITS && rho >= FIXED_MIN_RHO_DB
        }
    }

    fn decoded_tone_ratio(o: &SoftObservations, tones: &ToneFrame) -> f64 {
        let (mut decoded, mut other) = (0.0, 0.0);
        for k in 0..DATA_SYMBOLS {
            let tone = tones[SYNC_SYMBOLS + k] as usize;
            for q in 0..4 {
                let e = c64(o.full[k][q]).norm_sqr();
                if q == tone {
                    decoded += e;
                } else {
                    other += e;
                }
            }
        }
        if other > 0.0 {
            decoded / (other / 3.0)
        } else {
            1e9
        }
    }

    /// D9: the payload observations at (f, t).
    fn observe(&self, f: f64, t: i64) -> SoftObservations {
        let mut o = SoftObservations::default();
        let payload = t + SYNC_LENGTH_FD as i64;
        let y = self.mix(f, payload, DATA_SYMBOLS * N_SYM);
        for m in 0..DATA_SYMBOLS {
            let start = payload + (m * N_SYM) as i64;
            if start < 0 || start + N_SYM as i64 > WINDOW_FD {
                continue;
            }
            for k in 0..4 {
                let (mut a, mut d) = (Complex64::new(0.0, 0.0), Complex64::new(0.0, 0.0));
                for r in 0..N_SYM {
                    let v = c64(y[m * N_SYM + r]) * c64(self.t.tone_conj[k][r]);
                    if r < N_SYM / 2 {
                        a += v;
                    } else {
                        d += v;
                    }
                }
                let c = a + d;
                o.full[m][k] = Complex32::new(c.re as f32, c.im as f32);
                o.split[m][k] = (a.norm_sqr() + d.norm_sqr()).sqrt() as f32;
            }
        }
        o
    }

    /// The decoded frame's whole trajectory, coherent over four-symbol
    /// segments, at integer offsets [t_first, t_last] (every `t_step`).
    fn frame_metrics(
        &self,
        reference: &[Complex32],
        f: f64,
        t_first: i64,
        t_last: i64,
        t_step: usize,
    ) -> Vec<f64> {
        const SEGMENT: usize = 4 * N_SYM;
        let span = (t_last - t_first) as usize + FRAME_FD;
        let mut z = vec![Complex64::new(0.0, 0.0); span];
        let w = -TWO_PI * f / FD;
        let step = Complex64::new(w.cos(), w.sin());
        let mut rot = Complex64::new((w * t_first as f64).cos(), (w * t_first as f64).sin());
        for (i, zi) in z.iter_mut().enumerate() {
            let m = t_first + i as i64;
            if (0..BASEBAND_SIZE as i64).contains(&m) {
                *zi = c64(self.x[m as usize]) * rot;
            }
            rot *= step;
            if i & 1023 == 1023 {
                rot /= rot.norm();
            }
        }
        let mut out = vec![0.0; (t_last - t_first + 1) as usize];
        let mut t = t_first;
        while t <= t_last {
            let off = (t - t_first) as usize;
            let mut total = 0.0;
            let mut begin = 0;
            while begin < FRAME_FD {
                let end = FRAME_FD.min(begin + SEGMENT);
                let mut sum = Complex64::new(0.0, 0.0);
                for i in begin..end {
                    sum += z[off + i] * c64(reference[i]).conj();
                }
                total += sum.norm_sqr();
                begin += SEGMENT;
            }
            out[off] = total;
            t += t_step as i64;
        }
        out
    }

    fn parabola_offset(below: f64, centre: f64, above: f64) -> f64 {
        let den = below - 2.0 * centre + above;
        if !(den < 0.0) {
            return 0.0;
        }
        let offset = 0.5 * (below - above) / den;
        if offset.abs() <= 1.0 { offset } else { 0.0 }
    }

    /// The reported frequency and fractional start (fd samples) of a decoded
    /// frame; it also places the subtraction and the assembly times.
    fn report_estimate(&self, tones: &ToneFrame, f0: f64, t0: i64) -> (f64, f64) {
        let reference = gfsk_baseband(tones, N_SYM as f64, 0.0, 0, FRAME_FD, 0.0);
        // coarse: +-24 samples in steps of 3, +-5 Hz in 0.5 Hz
        let mut best = -1.0;
        let mut best_f = f0;
        let mut best_t = t0.max(0);
        let lo = (t0 - 24).max(0);
        let hi = lo.max(t0 + 24);
        for k in -10..=10 {
            let f = f0 + 0.5 * k as f64;
            let metrics = self.frame_metrics(&reference, f, lo, hi, 3);
            let mut t = lo;
            while t <= hi {
                let m = metrics[(t - lo) as usize];
                if m > best {
                    best = m;
                    best_f = f;
                    best_t = t;
                }
                t += 3;
            }
        }
        // fine: +-2 samples, +-0.5 Hz in 0.1 Hz
        let flo = (best_t - 3).max(0);
        let fhi = best_t + 3;
        let coarse_f = best_f;
        let mut rows = Vec::with_capacity(13);
        for k in -6..=6 {
            let metrics = self.frame_metrics(&reference, coarse_f + 0.1 * k as f64, flo, fhi, 1);
            for t in flo.max(best_t - 2)..=fhi.min(best_t + 2) {
                let m = metrics[(t - flo) as usize];
                if m > best {
                    best = m;
                    best_f = coarse_f + 0.1 * k as f64;
                    best_t = t;
                }
            }
            rows.push(metrics);
        }
        let best_k = ((best_f - coarse_f) / 0.1).round() as i64;
        let row = |k: i64| &rows[(k + 6) as usize];
        let ti = (best_t - flo) as usize;
        let mut f_out = best_f;
        if best_k > -6 && best_k < 6 {
            f_out += 0.1
                * Self::parabola_offset(row(best_k - 1)[ti], row(best_k)[ti], row(best_k + 1)[ti]);
        }
        let mut t_out = best_t as f64;
        if best_t > flo && best_t < fhi {
            let r = row(best_k);
            t_out += Self::parabola_offset(r[ti - 1], r[ti], r[ti + 1]);
        }
        (f_out, t_out)
    }

    /// D12.1: subtract a known frame placed at fractional fd offset `t`,
    /// with its gain smoothed by a cos^2 kernel (prefix sums, exact).
    fn subtract(&mut self, tones: &ToneFrame, f: f64, t: f64) {
        let first = t.floor() as i64;
        let count = FRAME_FD + 2;
        let mut psi = gfsk_baseband(
            tones,
            N_SYM as f64,
            t - first as f64,
            0,
            count,
            N_SYM as f64 / 8.0,
        );
        let w = TWO_PI * f / FD;
        for (i, v) in psi.iter_mut().enumerate() {
            let a = remainder(w * (first + i as i64) as f64, TWO_PI);
            *v *= Complex32::new(a.cos() as f32, a.sin() as f32);
        }
        let lo = (-first).max(0) as usize;
        let hi = (count as i64).min(WINDOW_FD - first);
        if hi <= lo as i64 {
            return;
        }
        let hi = hi as usize;
        self.map_stale = true;
        let mut gain = vec![Complex64::new(0.0, 0.0); count];
        for i in lo..hi {
            gain[i] = c64(self.x[(first + i as i64) as usize]) * c64(psi[i]).conj();
        }
        let n = N_SYM as i64;
        let unit = |k: i64| {
            let r = k.rem_euclid(2 * n);
            let a = std::f64::consts::PI * r as f64 / n as f64;
            Complex64::new(a.cos(), a.sin())
        };
        let mut p0 = vec![Complex64::new(0.0, 0.0); count + 1];
        let mut pm = p0.clone();
        let mut pp = p0.clone();
        for k in 0..count {
            let g = if (lo..hi).contains(&k) {
                gain[k]
            } else {
                Complex64::new(0.0, 0.0)
            };
            let e = unit(k as i64);
            p0[k + 1] = p0[k] + g;
            pm[k + 1] = pm[k] + g * e.conj();
            pp[k + 1] = pp[k] + g * e;
        }
        let scale = 1.0 / (2.0 * self.t.smoother_sum);
        for i in lo..hi {
            let a = (i as i64 + 1 - n).max(0) as usize;
            let b = (count as i64).min(i as i64 + 2 + n) as usize;
            if a >= b {
                continue;
            }
            let phase = unit(i as i64 + 1);
            let sum = (p0[b] - p0[a])
                + 0.5 * phase * (pm[b] - pm[a])
                + 0.5 * (pp[b] - pp[a]) * phase.conj();
            let r = scale * sum * c64(psi[i]);
            let xi = &mut self.x[(first + i as i64) as usize];
            *xi -= Complex32::new(r.re as f32, r.im as f32);
        }
    }

    /// Decode, check, test for duplicates, subtract and assemble one
    /// candidate. True if it decoded (fresh or duplicate).
    fn attempt(&mut self, w: &mut WindowState, channel: &Channel, f: f64, t: i64) -> bool {
        let observations = self.observe(f, t);
        let fec = decode_tiers(&observations);
        self.work.fec_hypotheses += fec.crc_hypotheses;
        let Some(payload) = fec.payload else {
            return false;
        };
        let Ok(atom) = decode_frame_payload(&payload) else {
            return false;
        };
        let Ok(encoded) = encode_payload(&payload) else {
            return false;
        };
        let required = if channel.extension {
            MIN_DECODED_TONE_RATIO_EXTENSION
        } else {
            MIN_DECODED_TONE_RATIO
        };
        if Self::decoded_tone_ratio(&observations, &encoded.tones) < required {
            return false;
        }
        self.work.frames_decoded += 1;
        if self.bounded && self.work.frames_decoded > MAX_DECODED_FRAMES {
            self.fail(format!(
                "receiver work budget exceeded: more than {MAX_DECODED_FRAMES} decoded frames in one reception"
            ));
            return false;
        }
        let (text, space_after) = if atom.atom == "TEXT5" {
            let t: Vec<u8> = atom
                .text5_raw
                .bytes()
                .map(|c| if c == b' ' { STAND_IN } else { c })
                .collect();
            (t, false)
        } else {
            (atom.rendering.into_bytes(), true)
        };
        // a frame already assembled (a look-back re-decode) keeps its estimate
        let search_s = (w.p as f64 + 2.0 * t as f64) / FS;
        let known = self
            .assembler
            .history()
            .find(|old| {
                old.payload == payload
                    && (old.frequency_hz - f).abs() < STAMP_DF_HZ
                    && (old.seconds() - search_s).abs() < STAMP_DT_S
            })
            .map(|old| (old.frequency_hz, (old.start_12k - w.p as f64) / 2.0));
        let (rf, rt) = known.unwrap_or_else(|| self.report_estimate(&encoded.tones, f, t));
        let entry = FrameEntry {
            frequency_hz: rf,
            start_12k: w.p as f64 + 2.0 * rt,
            payload,
            text,
            space_after,
            eom: atom.end_of_message,
        };
        let seconds = entry.seconds();
        // D13.1 a, b
        let verdict = w.duplicates.test(&entry.text, rf, seconds, !channel.rx);
        if verdict != Verdict::Fresh {
            self.work.duplicate_frames += 1;
            if verdict == Verdict::RxRepeat {
                w.duplicates.add_decoded(&entry.text, seconds);
            }
            return true;
        }
        w.duplicates.add_decoded(&entry.text, seconds);
        // D12.1
        self.subtract(&encoded.tones, rf, rt);
        self.work.subtractions += 1;
        w.subtracted = true;
        if w.record && w.recorded.len() < MAX_RECORDED_SUBTRACTIONS {
            w.recorded.push(RecordedSignal {
                frequency_hz: rf,
                start_12k: entry.start_12k,
                tones: encoded.tones,
            });
        }
        if channel.rx {
            w.duplicates.add_rx(rf, seconds);
        }
        let (merged, done) = self.assembler.merge(&entry, &mut self.work);
        debug_assert!(
            merged != MergeResult::Refused || self.assembler.open().len() >= MAX_OPEN_MESSAGES
        );
        if let Some(m) = done {
            self.complete(&m);
        }
        true
    }

    /// D13.4: after a pass that decoded nothing, the next frame of an open
    /// message in this channel where it should be.
    fn retry(&mut self, w: &mut WindowState, channel: &Channel) {
        let window_s = w.p as f64 / FS;
        let found = self.assembler.open().iter().find_map(|m| {
            if m.f_last < channel.center_hz - channel.half_width_hz
                || m.f_last > channel.center_hz + channel.half_width_hz
                || ((window_s - m.t_last) - FRAME_PERIOD_S).abs() > RETRY_TIMING_S
            {
                return None;
            }
            let x = m.t_last + FRAME_PERIOD_S - window_s;
            (x >= 0.0).then_some((m.f_last, x))
        });
        let Some((f, x)) = found else {
            return;
        };
        if w.records >= MAX_WINDOW_CANDIDATES {
            return;
        }
        w.records += 1;
        self.work.candidates += 1;
        self.work.retries += 1;
        self.attempt(w, channel, f, (x * FD).round() as i64);
    }

    /// One pass of one channel: D6 picks, D7 refinement, D8 gate.
    fn channel_pass(
        &mut self,
        w: &mut WindowState,
        channel: &Channel,
        mut exclusion: Option<&mut Vec<u8>>,
    ) {
        let mut decoded = false;
        for _ in 0..channel.picks {
            if self.failure.is_some() || w.records >= MAX_WINDOW_CANDIDATES {
                return;
            }
            let mut best = -1.0f32;
            let (mut best_b, mut best_s) = (-1i32, -1i32);
            for s in 0..MAP_STEPS {
                if w.p + 2 * s as i64 * MAP_STEP_FD - LEAD_12K < -SEARCH_REACH_12K {
                    continue;
                }
                for b in channel.b_lo..=channel.b_hi {
                    let i = self.map_index(s, b);
                    if exclusion.as_ref().is_some_and(|e| e[i] != 0) {
                        continue;
                    }
                    let g = self.map[i];
                    if g > best {
                        best = g;
                        best_b = b;
                        best_s = s as i32;
                    }
                }
            }
            if best_b < 0 {
                return; // fully excluded
            }
            let s_lo = (best_s - EXCLUDE_STEPS_BELOW).max(0);
            let s_hi = (best_s + EXCLUDE_STEPS_ABOVE).min(MAP_STEPS as i32 - 1);
            let b_lo = channel.b_lo.max(best_b - EXCLUDE_BINS_BELOW);
            let b_hi = channel.b_hi.min(best_b + EXCLUDE_BINS_ABOVE);
            for s in s_lo..=s_hi {
                for b in b_lo..=b_hi {
                    let i = self.map_index(s as usize, b);
                    match exclusion.as_mut() {
                        Some(e) => e[i] = 1,
                        None => self.map[i] = 0.0,
                    }
                }
            }
            w.records += 1;
            self.work.candidates += 1;
            let mut f = best_b as f64 * FBIN;
            let mut t = best_s as i64 * MAP_STEP_FD;
            if channel.rx || channel.extension {
                (f, t) = self.refine(f, t as f64 / FD);
            }
            if !self.gate(f, t, channel) {
                continue;
            }
            self.work.gated_candidates += 1;
            if self.attempt(w, channel, f, t) {
                decoded = true;
            }
        }
        if !decoded && self.failure.is_none() {
            self.retry(w, channel);
        }
    }

    fn zero_rx_vicinity(&mut self, w: &WindowState) {
        for &(f, _) in w.duplicates.rx() {
            let centre = (f / FBIN).round() as i32;
            for b in self.t.union_lo.max(centre - VICINITY_BINS)
                ..=self.t.union_hi.min(centre + VICINITY_BINS)
            {
                for s in 0..MAP_STEPS {
                    let i = self.map_index(s, b);
                    self.map[i] = 0.0;
                }
            }
        }
    }

    /// D1: one window evaluation, forward (`record`: its subtractions are
    /// kept for look-backs) or a look-back with `lookback` subtracted first.
    fn analyze(
        &mut self,
        p: i64,
        lookback: Option<&RecordedSignal>,
        record: bool,
    ) -> Vec<RecordedSignal> {
        if self.failure.is_some() {
            return Vec::new();
        }
        self.evaluations += 1;
        if self.bounded && self.evaluations > MAX_WINDOW_EVALUATIONS {
            self.fail(format!(
                "receiver work budget exceeded: more than {MAX_WINDOW_EVALUATIONS} window evaluations in one reception"
            ));
            return Vec::new();
        }
        if !self.load_window(p) {
            self.work.silent_windows += 1;
            return Vec::new();
        }
        if let Some(l) = lookback {
            self.subtract(&l.tones, l.frequency_hz, (l.start_12k - p as f64) / 2.0);
        }
        let mut w = WindowState {
            p,
            records: 0,
            duplicates: WindowDuplicates::default(),
            record,
            recorded: Vec::new(),
            subtracted: false,
            lookback: lookback.is_some(),
        };
        let t = self.t;
        let channels = &t.channels;
        // the Rx channel's round
        self.build_map();
        let mut exclusion = vec![0u8; MAP_STEPS * self.map_bins()];
        for pass in 1..=2 {
            if pass == 2 {
                if !w.subtracted {
                    break;
                }
                self.build_map();
            }
            w.subtracted = false;
            exclusion.fill(0);
            self.channel_pass(&mut w, &channels[0], Some(&mut exclusion));
        }
        // the fixed channels' round: pass 1 rebuilds the map only after a
        // subtraction, pass 2 always
        for pass in 1..=2 {
            if pass == 2 || self.map_stale {
                self.build_map();
            }
            w.subtracted = false;
            self.zero_rx_vicinity(&w);
            for c in &channels[1..] {
                self.channel_pass(&mut w, c, None);
            }
            if !w.subtracted {
                break;
            }
        }
        let _ = w.lookback;
        w.recorded
    }

    fn complete(&mut self, m: &OpenMessage) {
        let text = publication_text(&m.text);
        if text.is_empty() {
            return;
        }
        let frequency = m.f_last.round() as i64;
        if frequency < SEARCH_LOW_HZ as i64 || frequency > SEARCH_HIGH_HZ as i64 {
            return;
        }
        self.out.push(Message {
            text,
            frequency_hz: frequency as i32,
            start_sample: m.first.start_12k.round() as i64 - LEAD_12K,
            frames: m.frames,
        });
        self.results += 1;
        if self.bounded && self.results > MAX_RESULTS {
            self.fail(format!(
                "completed message count exceeds the {MAX_RESULTS}-result bound"
            ));
        }
    }
}

fn c64(v: Complex32) -> Complex64 {
    Complex64::new(v.re as f64, v.im as f64)
}

/// One finite recording (12 kHz mono, at most 120 s) as one reception, with
/// jtty-cuf's work bounds; messages sorted by start, frequency, text.
pub fn receive_messages(pcm: &[f32]) -> Result<Vec<Message>, String> {
    if pcm.is_empty() {
        return Err("receiver input is empty".into());
    }
    if pcm.len() > MAX_INPUT_SAMPLES {
        return Err("receiver input exceeds the 1,440,000-sample bound".into());
    }
    if pcm.iter().any(|v| !v.is_finite()) {
        return Err("receiver input contains a non-finite sample".into());
    }
    let mut r = Receiver::new();
    r.bounded = true;
    let mut out = r.push(pcm);
    out.extend(r.finish());
    if let Some(e) = r.failure {
        return Err(e);
    }
    out.sort_by(|a, b| {
        (a.start_sample, a.frequency_hz, &a.text).cmp(&(b.start_sample, b.frequency_hz, &b.text))
    });
    Ok(out)
}
