// SPDX-License-Identifier: GPL-3.0-or-later
//! LF time-code stations watched on the receiver (`[timecode]`,
//! `doc/api.md` §8.5): one channelizer bin (60 kS/s complex) holds several
//! (at 60 kHz: MSF, RBU and DCF77). Per station:
//!
//! * a mixer puts its carrier at 0 Hz, a 128-tap filter (1 kHz) and
//!   decimation by 15 give 4 kS/s, shifted up 1 kHz as USB audio for the
//!   same decoder
//!   the page runs (`web888_timecode::Receiver`);
//! * each second's mean of the baseband: the carrier's level (dBFS in
//!   ~1 Hz) and phase; the phase's slope over the last minute is the
//!   carrier's frequency error, which is the ADC clock's (the carriers
//!   come from atomic clocks): `adc_ppm`, with no network involved;
//! * each minute decoded: how late its start is by the receiver's
//!   timebase (the path from the station, the receiver's delays and the
//!   clock's error).
//!
//! Once a second the bin's spectrum (4096 points) gives the noise floor
//! (the median over the clean ±20 kHz, dBFS/Hz): per minute its median
//! and 90th percentile over the seconds (local bursts, such as power-line
//! meter traffic, raise the latter).
//!
//! The system's clocks against the ADC's (a few ppb, by the carriers):
//! each minute the bin's frame count against `CLOCK_MONOTONIC_RAW` (the
//! PS crystal as it runs, no NTP) and `CLOCK_REALTIME` (as NTP steers
//! it), each at the run that arrived soonest (least scheduling delay);
//! with the kernel's frequency correction and the SoC's temperature: how
//! the system clock would drift without NTP.
//!
//! At each UTC minute: a [`Report`] per station, to the metrics
//! ([`snapshot`]) and, with `log`, a JSON line to `timecode.log`.

use crate::session::Shared;
use num_complex::{Complex32, Complex64};
use std::f64::consts::PI;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use web888_config::Timecode;
use web888_dsp::binrx::{BinTap, TapMsg};
use web888_radio::Iq32;
use web888_radio::chan::RX_SINE_FULL_SCALE;
use web888_timecode::{Event, Minute, Receiver, Station};

/// The decoders' audio rate and the carrier's tone in it.
const FS: f64 = 4_000.0;
const TONE: f64 = 1000.0;
const TAPS: usize = 128;
/// The bin's clean half-width (Hz).
const CLEAN_HZ: f64 = 21_000.0;
const FFT_N: usize = 4096;
const LOG_MAX: u64 = 10 << 20;
/// Audio handed to a decoder at a time (1 s: each push sorts its last
/// 30 s of envelope, so pushes cost by their number).
const CHUNK: usize = FS as usize;
/// Seconds of the stream's reads that measure the system clock "now"
/// (`Timebase::recent`): ~20 µs of read jitter left, 10 s old.
pub const RECENT_S: usize = 20;
/// The carrier over the floor (dB in 1 Hz) for its frequency to count.
const MIN_SNR: f64 = 20.0;

/// One station's minute.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Report {
    /// UTC (s) of the minute's start (the receiver's clock).
    pub t: i64,
    pub station: String,
    /// The centre of the bin it is watched in (kHz).
    pub bin_khz: f64,
    /// The carrier: its mean level over the minute's seconds (dBFS, each
    /// second's coherent mean: ~1 Hz) and over the noise floor (dB in
    /// 1 Hz).
    pub carrier_dbfs: Option<f64>,
    pub snr_db_hz: Option<f64>,
    /// The keying's depth (dB), as the page shows it.
    pub keying_db: f64,
    /// A minute decoded and its checks passed; confirmed by an earlier one.
    /// Only a confirmed minute counts: the checks alone pass wrong times
    /// (DCF77's parity, hours off, seen on the unit).
    pub ok: bool,
    pub confirmed: bool,
    /// Seconds not read in the decoded minute.
    pub unread: Option<usize>,
    /// The confirmed minute's start, late by the receiver's clock (ms):
    /// the clock as it was at `at` (lately, not over the timebase's
    /// minutes-long fit: a clock being steered would be measured late).
    pub late_ms: Option<f64>,
    /// When the clock was as `late_ms` and `offset_ms` say (system
    /// time, s): the middle of the last [`RECENT_S`] seconds' reads.
    #[serde(skip)]
    pub at: Option<f64>,
    /// The ADC clock's error (ppm, + fast) from the carrier's frequency
    /// over the last minute.
    pub adc_ppm: Option<f64>,
    /// The same from the timebase (against the system clock: NTP).
    pub tb_ppm: Option<f64>,
    /// The bin's noise floor over the minute (dBFS/Hz).
    pub floor_dbfs_hz: Option<f64>,
    pub floor_p90_dbfs_hz: Option<f64>,
    /// The system's clocks over the last minute against the ADC clock
    /// (ppm, + fast; add `adc_ppm` for their error against UTC): the PS
    /// crystal as it runs (`CLOCK_MONOTONIC_RAW`) and the clock NTP
    /// steers (`CLOCK_REALTIME`).
    pub raw_ppm: Option<f64>,
    pub sys_ppm: Option<f64>,
    /// The kernel's frequency correction (NTP's: ppm) and the SoC's
    /// temperature (°C).
    pub kernel_freq_ppm: Option<f64>,
    pub temp_c: Option<f64>,
    /// The system clock's error by this station (ms, true minus system
    /// time: the confirmed minute's lateness less the path from the
    /// transmitter and the decoder's own delay) and its scatter over the
    /// last minutes (ms, a standard deviation from their mean absolute
    /// deviation): what goes to [`crate::timesync`].
    pub offset_ms: Option<f64>,
    pub sigma_ms: Option<f64>,
    /// The station is shed (the monitor short of CPU, the skimmers already
    /// all shed): not decoded for now.
    pub shed: bool,
    /// The monitor's CPU over the last minute (% of a core).
    pub cpu_pct: Option<f64>,
}

/// The system's clocks against the bin's frame count, a minute at a time.
#[derive(Default)]
struct ClockMeter {
    /// This minute's soonest run: its offset (raw s − frames / rate), end
    /// frame, raw and real time.
    best: Option<(f64, u64, f64, f64)>,
    prev: Option<(u64, f64, f64)>,
}

impl ClockMeter {
    fn run(&mut self, end: u64, rate: f64) {
        let raw = crate::sysstat::clock_s(libc::CLOCK_MONOTONIC_RAW);
        let rt = crate::sysstat::clock_s(libc::CLOCK_REALTIME);
        let off = raw - end as f64 / rate;
        if self.best.is_none_or(|b| off < b.0) {
            self.best = Some((off, end, raw, rt));
        }
    }

    /// The minute's (raw, real) ppm against the ADC clock.
    fn minute(&mut self, rate: f64) -> (Option<f64>, Option<f64>) {
        let Some((_, f, raw, rt)) = self.best.take() else {
            return (None, None);
        };
        let prev = self.prev.replace((f, raw, rt));
        let Some((f0, raw0, rt0)) = prev else {
            return (None, None);
        };
        let adc = (f - f0) as f64 / rate;
        // at least half a minute (else a restart's first minute)
        if !(30.0..200.0).contains(&adc) {
            return (None, None);
        }
        let r = |v: f64| ((v / adc - 1.0) * 1e6 * 1e4).round() / 1e4;
        (Some(r(raw - raw0)), Some(r(rt - rt0)))
    }
}

/// What the metrics show: the last report per station, and the minute
/// counts.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub reports: Vec<Report>,
    /// Per station: minutes confirmed, decoded but not confirmed, not
    /// decoded.
    pub counts: Vec<[u64; 3]>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// The monitor's state (none: it doesn't run).
pub fn snapshot() -> Option<State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// A station's chain: mixer, decimator, decoder, the carrier's seconds.
struct Chain {
    station: Station,
    fc: f64,
    rot: Complex64,
    step: Complex64,
    /// The mixer as a table when its frequency repeats within a few
    /// hundred samples (a bin at 60 kHz: MSF 1, RBU 9, DCF77 24 samples),
    /// exact with no drift; else the rotator.
    table: Vec<Complex32>,
    ti: usize,
    /// The mixed samples: the filter's history and the run's (linear, so
    /// each output's window is a slice); `next`: the index of the next
    /// output's newest sample.
    buf: Vec<Complex32>,
    next: usize,
    /// Decimated samples so far.
    n: u64,
    up: Complex64,
    up_step: Complex64,
    rx: Receiver,
    audio: Vec<f32>,
    /// The current second's sum and count; the seconds' (mid sample, mean).
    sec: (Complex64, usize),
    /// The second's power (EFR: FSK has no steady carrier to average).
    sec_pow: f64,
    secs: std::collections::VecDeque<(f64, Complex64)>,
    /// This UTC minute's: the carrier's seconds (amplitude), the minutes
    /// decoded.
    amps: Vec<f64>,
    minutes: Vec<Minute>,
    /// The path from the transmitter and the decoder's own delay (ms;
    /// none: no site or no receiver locator).
    delay_ms: Option<f64>,
    /// The last confirmed minutes' lateness (ms), for the scatter.
    lates: std::collections::VecDeque<f64>,
    /// Not decoded for now (shedding).
    shed: bool,
}

impl Chain {
    fn new(station: Station, fc: f64, centre: f64, rate: f64) -> Self {
        let w = -2.0 * PI * (fc - centre) / rate;
        let u = 2.0 * PI * TONE / FS;
        Chain {
            station,
            fc,
            rot: Complex64::new(1.0, 0.0),
            step: Complex64::new(w.cos(), w.sin()),
            table: (1..=512)
                .find(|&p| {
                    let c = (fc - centre) * p as f64 / rate;
                    (c - c.round()).abs() < 1e-9
                })
                .map(|p| {
                    (0..p)
                        .map(|k| {
                            let a = w * k as f64;
                            Complex32::new(a.cos() as f32, a.sin() as f32)
                        })
                        .collect()
                })
                .unwrap_or_default(),
            ti: 0,
            buf: vec![Complex32::default(); TAPS - 1],
            // the first output at the decimation's last input
            next: TAPS - 2 + (rate / FS).round() as usize,
            n: 0,
            up: Complex64::new(1.0, 0.0),
            up_step: Complex64::new(u.cos(), u.sin()),
            rx: Receiver::new(station, FS, TONE, 0.0),
            audio: Vec::with_capacity(CHUNK + 4096),
            sec: (Complex64::default(), 0),
            sec_pow: 0.0,
            secs: Default::default(),
            amps: Vec::new(),
            minutes: Vec::new(),
            delay_ms: None,
            lates: Default::default(),
            shed: false,
        }
    }

    fn push(&mut self, iq: &[Complex32], decim: usize, taps: &[f32]) {
        // the mixer over the run, in a loop of its own (locals: the
        // per-sample cost on the A9 was in the bookkeeping, not the maths)
        let start = self.buf.len();
        self.buf.resize(start + iq.len(), Complex32::default());
        let out = &mut self.buf[start..];
        if self.table.is_empty() {
            let (mut rot, step) = (self.rot, self.step);
            for (o, &x) in out.iter_mut().zip(iq) {
                *o = x * Complex32::new(rot.re as f32, rot.im as f32);
                rot *= step;
            }
            self.rot = rot;
        } else {
            let t = &self.table;
            let mut ti = self.ti;
            for (o, &x) in out.iter_mut().zip(iq) {
                *o = x * t[ti];
                ti += 1;
                if ti == t.len() {
                    ti = 0;
                }
            }
            self.ti = ti;
        }
        while self.next < self.buf.len() {
            let w0 = self.next + 1 - TAPS;
            self.next += decim;
            let acc = web888_dsp::fir::dot_real(&self.buf[w0..w0 + TAPS], taps);
            let y = Complex64::new(acc.re as f64, acc.im as f64);
            self.n += 1;
            self.sec.0 += y;
            self.sec_pow += y.norm_sqr();
            self.sec.1 += 1;
            if self.sec.1 == FS as usize {
                let mean = self.sec.0 / FS;
                let mid = self.n as f64 - FS / 2.0;
                self.secs.push_back((mid, mean));
                while self.secs.len() > 60 {
                    self.secs.pop_front();
                }
                // the carrier's level: the coherent mean (~1 Hz); FSK's
                // tones: the power
                self.amps.push(if self.station.is_efr() {
                    (self.sec_pow / FS).sqrt()
                } else {
                    mean.norm()
                });
                self.sec = (Complex64::default(), 0);
                self.sec_pow = 0.0;
            }
            self.audio
                .push(((y * self.up).re / RX_SINE_FULL_SCALE) as f32);
            self.up *= self.up_step;
        }
        // keep the history only (now and then: the copy costs by its size)
        let keep = self.next + 1 - TAPS;
        if keep >= 4096 {
            self.buf.drain(..keep);
            self.next -= keep;
        }
        // back on the unit circle
        self.rot /= self.rot.norm();
        self.up /= self.up.norm();
        // the decoder in chunks (`CHUNK`): the bin comes in runs of a few
        // dozen samples
        if self.audio.len() < CHUNK {
            return;
        }
        self.rx.push(&self.audio);
        self.audio.clear();
        for e in self.rx.poll() {
            if let Event::Minute(m) = e {
                self.minutes.push(m);
            }
        }
    }

    /// The carrier's frequency error (ppm of its frequency, + when the
    /// ADC clock is fast) from the last minute's seconds: the phase's
    /// slope, weighted by each second's power.
    fn ppm(&self) -> Option<f64> {
        if self.secs.len() < 30 {
            return None;
        }
        let mut ph = Vec::with_capacity(self.secs.len());
        let mut last: Option<f64> = None;
        for &(t, m) in &self.secs {
            let mut a = m.arg();
            if let Some(l) = last {
                a = l + (a - l + PI).rem_euclid(2.0 * PI) - PI;
            }
            last = Some(a);
            ph.push((t / FS, a, m.norm_sqr()));
        }
        let sw: f64 = ph.iter().map(|p| p.2).sum();
        if sw <= 0.0 {
            return None;
        }
        let mx = ph.iter().map(|p| p.2 * p.0).sum::<f64>() / sw;
        let my = ph.iter().map(|p| p.2 * p.1).sum::<f64>() / sw;
        let sxx: f64 = ph.iter().map(|p| p.2 * (p.0 - mx).powi(2)).sum();
        let sxy: f64 = ph.iter().map(|p| p.2 * (p.0 - mx) * (p.1 - my)).sum();
        if sxx <= 0.0 {
            return None;
        }
        let hz = sxy / sxx / (2.0 * PI);
        // a carrier appearing low: the ADC clock fast
        Some(-hz / self.fc * 1e6)
    }
}

/// The decimating filter: windowed sinc (Blackman), 1 kHz cut-off at
/// `rate`, unity gain.
fn lowpass(rate: f64) -> Vec<f32> {
    let fc = 1000.0 / rate;
    let m = (TAPS - 1) as f64;
    let mut h: Vec<f64> = (0..TAPS)
        .map(|i| {
            let x = i as f64 - m / 2.0;
            let s = if x == 0.0 {
                2.0 * fc
            } else {
                (2.0 * PI * fc * x).sin() / (PI * x)
            };
            let w = 0.42 - 0.5 * (2.0 * PI * i as f64 / m).cos()
                + 0.08 * (4.0 * PI * i as f64 / m).cos();
            s * w
        })
        .collect();
    let g: f64 = h.iter().sum();
    h.iter_mut().for_each(|v| *v /= g);
    h.into_iter().map(|v| v as f32).collect()
}

/// Confirmed minutes kept per station for its scatter.
const LATES: usize = 15;
/// A station's scatter before it has `LATES / 3` minutes (ms): little
/// weight until it shows its own.
const SCATTER_PRIOR_MS: f64 = 3.0;
/// The least scatter assumed (ms).
const SCATTER_MIN_MS: f64 = 0.1;
/// A minute's deviation counts at most this much (ms).
const SCATTER_CAP_MS: f64 = 20.0;

/// The decoders' own delay per station plus the receiver's (ms): the
/// lateness of the confirmed minutes over the path from the transmitter,
/// medians over 2026-11-19/20 on the remote unit against its NTP-steered
/// clock (time3.google.com; so its error is in them too). RBU 3.88 ms
/// late on a 3.65 ms path, MSF 7.95 on 6.89, DCF77 9.78 on 6.82 (each
/// ±2-3 ms by the minute: their keying's 5 ms steps). EFR: the time
/// telegram's start bit after its 10 s mark, from the recorded hour of
/// 2026-11-19 (`efr134k`, against the recording's NTP timebase): DCF39
/// 1206.34 ms on a 5.77 ms path (±0.07 ms by the minute); HGA22 about
/// 1227 and DCF49 about 1250 ms, from their headers alone (±1-3 ms: too
/// weak to decode by day). Others: unmeasured.
fn decoder_ms(s: Station) -> f64 {
    match s {
        Station::Rbu => 0.23,
        Station::Msf => 1.06,
        Station::Dcf77 => 2.96,
        Station::Dcf39 => 1200.57,
        Station::Hga22 => 1220.0,
        Station::Dcf49 => 1244.2,
        // provisional, from GPS-timed KiwiSDR recordings (2026-11-21):
        // lateness less the ground-wave path, shifted by the decoders'
        // own lateness at 60 against 12 kS/s (synthetic: DCF77 -0.65 ms,
        // JJY +1.2, WWVB +0.1); a Kiwi near Mainflingen showed the Kiwi's
        // own delay ~0 (DCF77 there 0.06 ms off its synthetic lateness).
        // TDF: two Kiwis (near Paris, near Kaiserslautern) within 0.15 ms
        Station::Tdf => -16.4,
        // Nebraska, 703 km
        Station::Wwvb => 3.3,
        // 40 kHz: Saitama, 191 km; 60 kHz (2026-11-22, 21 JST): Hiroshima,
        // 224 km, 5.5 ms, and Okayama, 364 km, 7.6 ms (sky wave at night?)
        Station::Jjy => 7.5,
        // provisional: the decoder's own lateness on synthetic keying at
        // 12 kS/s (5.9 ms, median of 18 minutes) and the carrier's drop
        // 1.2 ms after the second (a GPS-timed KiwiSDR at Okayama,
        // 1655 km, 2026-11-22: the drop's 50 % point 6.7 ms late)
        Station::Bpc => 7.1,
        _ => 0.0,
    }
}

/// The time from a transmitter at `site` to `rx` (lat, lon °): the
/// great-circle distance at the ground wave's speed (ms).
fn path_ms(site: (f64, f64), rx: (f64, f64)) -> f64 {
    let r = |d: f64| d.to_radians();
    let (la1, lo1, la2, lo2) = (r(site.0), r(site.1), r(rx.0), r(rx.1));
    let h = ((la2 - la1) / 2.0).sin().powi(2)
        + la1.cos() * la2.cos() * ((lo2 - lo1) / 2.0).sin().powi(2);
    let km = 2.0 * h.sqrt().asin() * 6371.0;
    // c, slowed ~0.03 % over ground
    km / 299.792_458 / 0.9997
}

/// A standard deviation from the mean absolute deviation from the
/// median (ms; each deviation at most `SCATTER_CAP_MS`, so one wild
/// minute counts little). Not the median absolute deviation: DCF77's and
/// MSF's minutes come in two values 5 ms apart (their keying's steps),
/// and with most at one of them that reads near 0 (0.19 ms on the unit).
fn scatter(v: &std::collections::VecDeque<f64>) -> f64 {
    if v.len() < LATES / 3 {
        return SCATTER_PRIOR_MS;
    }
    let mut a: Vec<f64> = v.iter().copied().collect();
    let mid = a.len() / 2;
    let med = *a.select_nth_unstable_by(mid, f64::total_cmp).1;
    let mad = a
        .iter()
        .map(|x| (x - med).abs().min(SCATTER_CAP_MS))
        .sum::<f64>()
        / a.len() as f64;
    (1.2533 * mad).max(SCATTER_MIN_MS)
}

/// The stations and the bin, from the bin source.
pub struct Monitor {
    chains: Vec<Chain>,
    centre: f64,
    rate: f64,
    decim: usize,
    taps: Vec<f32>,
    /// The frame the chains started at.
    frame0: u64,
    next: Option<u64>,
    /// The floor: the second's frames for the FFT, the seconds' floors.
    fft: Arc<dyn rustfft::Fft<f32>>,
    window: Vec<f32>,
    fbuf: Vec<Complex32>,
    floors: Vec<f64>,
    since_fft: u64,
    /// Frames a second (the floor's FFT is at each second's start).
    rate_frames: u64,
    /// The run as complex floats (kept: no allocation per run).
    z: Vec<Complex32>,
    minute: Option<i64>,
    pub counts: Vec<[u64; 3]>,
    /// Breaks in the frames (data lost: the monitor behind).
    pub gaps: u64,
}

impl Monitor {
    /// `stations` in a bin at `centre` Hz of `rate` S/s; the ones whose
    /// carrier isn't within the clean part are refused.
    pub fn new(stations: &[String], centre: f64, rate: f64) -> Result<Self, String> {
        let decim = (rate / FS).round() as usize;
        if decim == 0 || (decim as f64 * FS - rate).abs() > 1e-6 {
            return Err(format!(
                "the bin's rate {rate} S/s isn't a multiple of {FS}"
            ));
        }
        let mut chains = Vec::new();
        for s in stations {
            let st = Station::from_name(s).ok_or_else(|| format!("unknown station {s:?}"))?;
            if st == Station::Wwv {
                return Err("WWV/WWVH is HF (a 100 Hz subcarrier in AM): not here".into());
            }
            // the nearest (JJY's 40 kHz is within reach of a bin at 60)
            let fc = st
                .carriers()
                .iter()
                .copied()
                .filter(|c| (c - centre).abs() <= CLEAN_HZ)
                .min_by(|a, b| (a - centre).abs().total_cmp(&(b - centre).abs()))
                .ok_or_else(|| format!("{s}: no carrier within {CLEAN_HZ} Hz of {centre} Hz"))?;
            chains.push(Chain::new(st, fc, centre, rate));
        }
        let window: Vec<f32> = (0..FFT_N)
            .map(|i| (0.5 - 0.5 * (2.0 * PI * i as f64 / FFT_N as f64).cos()) as f32)
            .collect();
        Ok(Monitor {
            counts: vec![[0; 3]; chains.len()],
            chains,
            centre,
            rate,
            decim,
            taps: lowpass(rate),
            frame0: 0,
            next: None,
            fft: web888_fft::plan(FFT_N, false),
            window,
            fbuf: Vec::with_capacity(FFT_N),
            floors: Vec::new(),
            since_fft: 0,
            rate_frames: rate.round() as u64,
            z: Vec::new(),
            minute: None,
            gaps: 0,
        })
    }

    /// The receiver's position (lat, lon °): the stations' paths, hence
    /// their offsets ([`Report::offset_ms`]).
    pub fn set_receiver(&mut self, rx: (f64, f64)) {
        for c in &mut self.chains {
            c.delay_ms = c
                .station
                .site(c.fc)
                .map(|s| path_ms(s, rx) + decoder_ms(c.station));
        }
    }

    /// The stations decoded: index, scatter of their minutes (ms).
    pub fn active(&self) -> Vec<(usize, f64)> {
        (0..self.chains.len())
            .filter(|&i| !self.chains[i].shed)
            .map(|i| (i, scatter(&self.chains[i].lates)))
            .collect()
    }

    /// Shed station `i` (or not): its name.
    pub fn set_shed(&mut self, i: usize, shed: bool) -> String {
        self.chains[i].shed = shed;
        self.chains[i].station.name().into()
    }

    pub fn any_shed(&self) -> bool {
        self.chains.iter().any(|c| c.shed)
    }

    /// Decode a shed station again (the least scattered first): its name.
    pub fn resume_one(&mut self) -> Option<String> {
        let i = (0..self.chains.len())
            .filter(|&i| self.chains[i].shed)
            .min_by(|&a, &b| {
                scatter(&self.chains[a].lates).total_cmp(&scatter(&self.chains[b].lates))
            })?;
        let c = &mut self.chains[i];
        // from scratch: its decoder missed the time it was shed
        let (d, l) = (c.delay_ms, std::mem::take(&mut c.lates));
        *c = Chain::new(c.station, c.fc, self.centre, self.rate);
        (c.delay_ms, c.lates) = (d, l);
        Some(c.station.name().into())
    }

    /// The stations and their carriers, for the log.
    pub fn describe(&self) -> String {
        self.chains
            .iter()
            .map(|c| format!("{} {:.3} kHz", c.station.name(), c.fc / 1e3))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Frames `first..` of the bin; `utc` maps a bin frame (fractional) to
    /// UTC, `recent` gives the system clock lately as (frame, time), for
    /// the stream's true rate in frames/s (`Timebase::recent`). The
    /// minutes that ended, one report per station.
    pub fn push(
        &mut self,
        first: u64,
        iq: &[Iq32],
        utc: impl Fn(f64) -> Option<f64>,
        recent: impl Fn(f64) -> Option<(f64, f64)>,
        tb_ppm: Option<f64>,
    ) -> Vec<Report> {
        if self.next != Some(first) {
            // a gap: start again
            if self.next.is_some() {
                self.gaps += 1;
            }
            for c in &mut self.chains {
                let (d, l, sh) = (c.delay_ms, std::mem::take(&mut c.lates), c.shed);
                *c = Chain::new(c.station, c.fc, self.centre, self.rate);
                (c.delay_ms, c.lates, c.shed) = (d, l, sh);
            }
            self.frame0 = first;
            self.fbuf.clear();
            self.since_fft = 0;
        }
        self.next = Some(first + iq.len() as u64);
        let mut z = std::mem::take(&mut self.z);
        z.clear();
        z.extend(iq.iter().map(|s| Complex32::new(s.i as f32, s.q as f32)));
        // the floor: FFT_N frames at the start of each second
        let mut i = 0;
        while i < z.len() {
            let left = (z.len() - i) as u64;
            let n = if self.since_fft < FFT_N as u64 {
                let n = left.min(FFT_N as u64 - self.since_fft);
                self.fbuf.extend_from_slice(&z[i..i + n as usize]);
                if self.fbuf.len() == FFT_N {
                    self.floor();
                }
                n
            } else {
                left.min(self.rate_frames - self.since_fft)
            };
            i += n as usize;
            self.since_fft += n;
            if self.since_fft >= self.rate_frames {
                self.since_fft = 0;
            }
        }
        for c in self.chains.iter_mut().filter(|c| !c.shed) {
            c.push(&z, self.decim, &self.taps);
        }
        self.z = z;
        // the minute by the receiver's clock
        let end = first + iq.len() as u64;
        let Some(now) = utc(end as f64) else {
            return vec![];
        };
        let m = (now / 60.0).floor() as i64;
        let prev = self.minute.replace(m);
        match prev {
            Some(p) if p != m => self.reports(p * 60, &utc, &recent, tb_ppm),
            _ => vec![],
        }
    }

    fn floor(&mut self) {
        let mut b: Vec<Complex32> = self
            .fbuf
            .iter()
            .zip(&self.window)
            .map(|(x, w)| x * w)
            .collect();
        self.fbuf.clear();
        self.fft.process(&mut b);
        let w2: f64 = self.window.iter().map(|w| (*w as f64).powi(2)).sum();
        let df = self.rate / FFT_N as f64;
        // the clean ±20 kHz (a channelizer bin), or 80 % of a narrower
        // stream (a KiwiSDR's 12 kS/s): bins 0..k and N-k..N
        let k = (20_000.0f64.min(0.4 * self.rate) / df) as usize;
        let mut p: Vec<f64> = b[..k]
            .iter()
            .chain(&b[FFT_N - k..])
            .map(|x| (x.norm_sqr() as f64) / w2 / self.rate)
            .collect();
        let mid = p.len() / 2;
        let med = *p.select_nth_unstable_by(mid, f64::total_cmp).1;
        let fs2 = RX_SINE_FULL_SCALE * RX_SINE_FULL_SCALE;
        self.floors.push(10.0 * (med / fs2).max(1e-30).log10());
    }

    fn reports(
        &mut self,
        t: i64,
        utc: &impl Fn(f64) -> Option<f64>,
        recent: &impl Fn(f64) -> Option<(f64, f64)>,
        tb_ppm: Option<f64>,
    ) -> Vec<Report> {
        let mut fl = std::mem::take(&mut self.floors);
        fl.sort_by(f64::total_cmp);
        let floor = (!fl.is_empty()).then(|| fl[fl.len() / 2]);
        let p90 = (!fl.is_empty()).then(|| fl[(fl.len() * 9 / 10).min(fl.len() - 1)]);
        // the stream's true rate: the atomic carriers' (their median), else
        // the timebase's (against the system clock), else nominal
        let mut cp: Vec<f64> = self
            .chains
            .iter()
            .filter(|c| c.station.atomic() && !c.amps.is_empty())
            .filter(|c| {
                let a = c.amps.iter().sum::<f64>() / c.amps.len() as f64;
                let db = 20.0 * (a / RX_SINE_FULL_SCALE).max(1e-15).log10();
                floor.is_some_and(|f| db - f > MIN_SNR)
            })
            .filter_map(|c| c.ppm())
            .collect();
        cp.sort_by(f64::total_cmp);
        let ppm = cp.get(cp.len() / 2).copied().or(tb_ppm).unwrap_or(0.0);
        let rate = self.rate * (1.0 + ppm * 1e-6);
        // the system clock lately: frames to its time
        let anchor = recent(rate);
        let now = |f: f64| anchor.map(|(fa, sa)| sa + (f - fa) / rate);
        let mut out = Vec::new();
        for (i, c) in self.chains.iter_mut().enumerate() {
            let amps = std::mem::take(&mut c.amps);
            let carrier = (!amps.is_empty()).then(|| {
                let a = amps.iter().sum::<f64>() / amps.len() as f64;
                20.0 * (a / RX_SINE_FULL_SCALE).max(1e-15).log10()
            });
            let minutes = std::mem::take(&mut c.minutes);
            let best = minutes
                .iter()
                .rev()
                .find(|m| m.ok && m.confirmed)
                .or_else(|| minutes.iter().rev().find(|m| m.ok))
                .or(minutes.last());
            let ok = best.is_some_and(|m| m.ok);
            let confirmed = best.is_some_and(|m| m.ok && m.confirmed);
            // the confirmed ones' lateness, their median (EFR: up to six
            // telegrams a minute)
            let mut lates: Vec<f64> = minutes
                .iter()
                .filter(|m| m.ok && m.confirmed)
                .filter_map(|m| {
                    // its first sample's bin frame: the decimated sample,
                    // back by the filter's delay
                    let f =
                        self.frame0 as f64 + m.t * FS * self.decim as f64 - (TAPS - 1) as f64 / 2.0;
                    now(f).or_else(|| utc(f)).map(|u| (u - m.utc as f64) * 1e3)
                })
                .collect();
            lates.sort_by(f64::total_cmp);
            let late_ms = (!lates.is_empty()).then(|| {
                let n = lates.len();
                let v = if n % 2 == 1 {
                    lates[n / 2]
                } else {
                    (lates[n / 2 - 1] + lates[n / 2]) / 2.0
                };
                (v * 100.0).round() / 100.0
            });
            let mut offset_ms = None;
            let mut sigma_ms = None;
            if let Some(l) = late_ms {
                c.lates.push_back(l);
                while c.lates.len() > LATES {
                    c.lates.pop_front();
                }
                // through the recent reads only (the fit lags a steered clock)
                offset_ms = c
                    .delay_ms
                    .filter(|_| anchor.is_some())
                    .map(|d| ((d - l) * 1e3).round() / 1e3);
                sigma_ms = Some((scatter(&c.lates) * 1e3).round() / 1e3);
            }
            if !c.shed {
                self.counts[i][match (ok, confirmed) {
                    (true, true) => 0,
                    (true, false) => 1,
                    _ => 2,
                }] += 1;
            }
            let r3 = |v: f64| (v * 1e3).round() / 1e3;
            let r1 = |v: f64| (v * 10.0).round() / 10.0;
            out.push(Report {
                t,
                station: c.station.name().into(),
                bin_khz: self.centre / 1e3,
                carrier_dbfs: carrier.map(r1),
                snr_db_hz: carrier.zip(floor).map(|(c, f)| r1(c - f)),
                keying_db: r1(c.rx.level as f64),
                ok,
                confirmed,
                unread: best.map(|m| m.symbols.chars().filter(|&s| s == '?').count()),
                late_ms,
                at: anchor.map(|a| a.1).filter(|_| late_ms.is_some()),
                // a carrier well above the floor only (else its phase is noise)
                adc_ppm: c
                    .ppm()
                    .filter(|_| c.station.atomic())
                    .filter(|_| carrier.zip(floor).is_some_and(|(c, f)| c - f > MIN_SNR))
                    .map(r3),
                tb_ppm: tb_ppm.map(r3),
                floor_dbfs_hz: floor.map(r1),
                floor_p90_dbfs_hz: p90.map(r1),
                raw_ppm: None,
                sys_ppm: None,
                kernel_freq_ppm: None,
                temp_c: None,
                offset_ms,
                sigma_ms,
                shed: c.shed,
                cpu_pct: None,
            });
        }
        out
    }
}

/// Start watching (`[timecode] enabled`): a thread on the bin tap.
/// `locator`: the receiver's (`[station]`), for the paths from the
/// transmitters. Stations whose carrier is in the clean part of the bin
/// at `centre_hz` are watched there, others in the bin nearest their
/// carrier (EFR's at 129-139 kHz: the 120 kHz bin): a bin each costs
/// its own floor and mixing.
pub fn spawn(
    sh: &Arc<Shared>,
    cfg: Timecode,
    locator: &str,
    state_dir: PathBuf,
) -> std::io::Result<()> {
    if !cfg.enabled {
        return Ok(());
    }
    let Some(tap) = sh.waterfalls.tap().cloned() else {
        eprintln!("timecode: needs the channelizer's bins; not watching");
        return Ok(());
    };
    let caps = tap.caps();
    let main = crate::wideiq::nearest_bin(&caps, cfg.centre_hz);
    let rx = crate::kiwi::locator_pos(locator);
    // the config's stations, else the region's
    let names: Vec<String> = if !cfg.stations.is_empty() {
        cfg.stations.clone()
    } else {
        match Station::for_region(&cfg.region, rx) {
            Some(v) => v.iter().map(|s| s.name().to_string()).collect(),
            None if cfg.region == "auto" => {
                eprintln!("timecode: region \"auto\" needs [station] locator; not watching");
                return Ok(());
            }
            None => {
                eprintln!("timecode: unknown region {:?}; not watching", cfg.region);
                return Ok(());
            }
        }
    };
    if names.is_empty() {
        eprintln!(
            "timecode: no station within reach ({}); not watching",
            cfg.region
        );
        return Ok(());
    }
    // the stations by bin, in the config's order
    let mut groups: Vec<(usize, Vec<String>)> = Vec::new();
    for name in &names {
        let Some(st) = Station::from_name(name) else {
            eprintln!("timecode: unknown station {name:?}; not watching it");
            continue;
        };
        let near = |b: usize| {
            st.carriers()
                .iter()
                .any(|c| (c - caps.freq(b)).abs() <= CLEAN_HZ)
        };
        // the main bin, then one already watched, then the nearest
        let bin = if near(main) {
            main
        } else if let Some(g) = groups.iter().find(|g| near(g.0)) {
            g.0
        } else {
            st.carriers()
                .iter()
                .map(|&c| crate::wideiq::nearest_bin(&caps, c))
                .find(|&b| near(b))
                .unwrap_or(main)
        };
        match groups.iter_mut().find(|g| g.0 == bin) {
            Some(g) => g.1.push(name.clone()),
            None => groups.push((bin, vec![name.clone()])),
        }
    }
    if rx.is_none() {
        eprintln!("timecode: no [station] locator: no offsets for the system clock");
    }
    let mut mons = Vec::new();
    for (bin, names) in groups {
        let centre = caps.freq(bin);
        match Monitor::new(&names, centre, caps.frame_rate) {
            Ok(mut m) => {
                if let Some(rx) = rx {
                    m.set_receiver(rx);
                }
                eprintln!(
                    "timecode: {} in the bin at {:.0} kHz",
                    m.describe(),
                    centre / 1e3
                );
                mons.push((bin, m));
            }
            Err(e) => eprintln!("timecode: {e}; not watching those"),
        }
    }
    if mons.is_empty() {
        return Ok(());
    }
    *STATE.lock().unwrap() = Some(State::default());
    let log = cfg.log.then(|| state_dir.join("timecode.log"));
    std::thread::Builder::new()
        .name("timecode".into())
        .spawn(move || run(tap, mons, log))?;
    Ok(())
}

/// Data lost this many times within `LOSS_WINDOW`, with no skimmer band
/// left to shed: the monitor sheds a station of its own.
const LOSSES_TO_SHED: usize = 3;
const LOSS_WINDOW: Duration = Duration::from_secs(600);
/// A shed station comes back after this long without a loss (and as
/// long after the last change).
const RESUME_AFTER: Duration = Duration::from_secs(1800);

/// The monitor's place in the load shedding: it ranks above the FT8/FT4
/// skimmers (their decoders run at idle priority, below it), so when it
/// loses data (its queue from the bin reader overflowed: it fell behind)
/// it asks them to shed bands ([`web888_skim::shed::press`]); with none
/// left to shed and the losses going on, it sheds its own noisiest
/// station, keeping one.
#[derive(Default)]
struct Shedding {
    gaps: u64,
    losses: std::collections::VecDeque<std::time::Instant>,
    last_change: Option<std::time::Instant>,
}

impl Shedding {
    fn check(&mut self, mons: &mut [(usize, Monitor)]) {
        use web888_skim::events::{Level, emit};
        let now = std::time::Instant::now();
        let since = |t: Option<std::time::Instant>| t.map_or(Duration::MAX, |t| now - t);
        let gaps: u64 = mons.iter().map(|m| m.1.gaps).sum();
        if gaps > self.gaps {
            self.gaps = gaps;
            web888_skim::shed::press();
            self.losses.push_back(now);
        }
        while self.losses.front().is_some_and(|&t| now - t > LOSS_WINDOW) {
            self.losses.pop_front();
        }
        if self.losses.len() >= LOSSES_TO_SHED
            && !web888_skim::shed::bands_left()
            && since(self.last_change) >= Duration::from_secs(60)
        {
            // the noisiest station of all, keeping one
            let on: Vec<(usize, usize, f64)> = mons
                .iter()
                .enumerate()
                .flat_map(|(k, m)| m.1.active().into_iter().map(move |(i, sc)| (k, i, sc)))
                .collect();
            if on.len() >= 2
                && let Some(&(k, i, _)) = on.iter().max_by(|a, b| a.2.total_cmp(&b.2))
            {
                let name = mons[k].1.set_shed(i, true);
                emit(
                    Level::Notice,
                    "timecode",
                    format!("shed {name}: the monitor is short of CPU"),
                );
                self.last_change = Some(now);
                self.losses.clear();
            }
        }
        if since(self.losses.back().copied()) >= RESUME_AFTER
            && since(self.last_change) >= RESUME_AFTER
            && let Some(m) = mons.iter_mut().find(|m| m.1.any_shed())
            && let Some(name) = m.1.resume_one()
        {
            emit(Level::Notice, "timecode", format!("resume {name}"));
            self.last_change = Some(now);
        }
    }
}

fn thread_cpu_s() -> f64 {
    crate::sysstat::clock_s(libc::CLOCK_THREAD_CPUTIME_ID)
}

fn run(tap: BinTap, mut mons: Vec<(usize, Monitor)>, log: Option<PathBuf>) {
    let (handle, rx) = tap.subscribe();
    handle.want(mons.iter().map(|m| m.0).collect(), 0.0);
    let mut iq: Vec<Iq32> = Vec::new();
    let mut clocks = ClockMeter::default();
    let mut shedding = Shedding::default();
    let rate = tap.caps().frame_rate;
    let mut cpu0 = (thread_cpu_s(), std::time::Instant::now());
    // each bin's last reports
    let mut last: Vec<Vec<Report>> = vec![Vec::new(); mons.len()];
    loop {
        let m = match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(m) => m,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => return,
        };
        let TapMsg::Frames(runs) = m else { continue };
        // the lock only for each look-up: the bin reader takes it too
        let ppm = tap.timebase().info().adc_ppm;
        let mut reports = Vec::new();
        let mut end = None;
        for (k, (bin, mon)) in mons.iter_mut().enumerate() {
            // a message's runs as one push where they're contiguous (a few
            // runs of a few dozen frames each), one clock sample and
            // timebase look-up per message: per run, these cost more than
            // the DSP
            let mut first: Option<u64> = None;
            let mut e = 0u64;
            iq.clear();
            for f in runs.iter() {
                let Some(idx) = f.bins.iter().position(|b| b == bin) else {
                    continue;
                };
                if let Some(f0) = first
                    && f.first != e
                {
                    reports.extend(
                        mon.push(
                            f0,
                            &iq,
                            |fr| tap.timebase().utc(fr),
                            |r| tap.timebase().recent(RECENT_S, r),
                            ppm,
                        )
                        .into_iter()
                        .map(|r| (k, r)),
                    );
                    iq.clear();
                    first = None;
                }
                first.get_or_insert(f.first);
                let n = f.bins.len();
                iq.extend(f.iq.chunks_exact(n).take(f.count).map(|fr| fr[idx]));
                e = f.first + f.count as u64;
            }
            let Some(f0) = first else { continue };
            end = Some(e);
            reports.extend(
                mon.push(
                    f0,
                    &iq,
                    |fr| tap.timebase().utc(fr),
                    |r| tap.timebase().recent(RECENT_S, r),
                    ppm,
                )
                .into_iter()
                .map(|r| (k, r)),
            );
        }
        let Some(end) = end else { continue };
        clocks.run(end, rate);
        shedding.check(&mut mons);
        if reports.is_empty() {
            continue;
        }
        // a minute's end: the clocks once for all bins (the first bin's
        // report starts the minute)
        let (raw, sys) = if reports.iter().any(|r| r.0 == 0) {
            clocks.minute(rate)
        } else {
            (None, None)
        };
        let kf = crate::sysstat::kernel_freq_ppm().map(|v| (v * 1e3).round() / 1e3);
        let temp = crate::sysstat::temperature().map(|t| (t as f64 * 10.0).round() / 10.0);
        let cpu1 = (thread_cpu_s(), std::time::Instant::now());
        let wall = (cpu1.1 - cpu0.1).as_secs_f64();
        let cpu = (wall > 30.0).then(|| ((cpu1.0 - cpu0.0) / wall * 1e4).round() / 100.0);
        if cpu.is_some() {
            cpu0 = cpu1;
        }
        let mut fresh: Vec<Vec<Report>> = vec![Vec::new(); mons.len()];
        for (k, mut r) in reports {
            (r.raw_ppm, r.sys_ppm, r.kernel_freq_ppm, r.temp_c, r.cpu_pct) =
                (raw, sys, kf, temp, cpu);
            if let (Some(o), Some(sg)) = (r.offset_ms, r.sigma_ms) {
                crate::timesync::submit(crate::timesync::Sample {
                    source: r.station.clone(),
                    offset: o / 1e3,
                    at: r.at.unwrap_or(r.t as f64),
                    sigma: sg / 1e3,
                });
            }
            fresh[k].push(r);
        }
        for (k, f) in fresh.into_iter().enumerate() {
            if f.is_empty() {
                continue;
            }
            if let Some(p) = &log {
                write_log(p, &f);
            }
            last[k] = f;
        }
        let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
        *s = Some(State {
            reports: last.concat(),
            counts: mons.iter().flat_map(|m| m.1.counts.clone()).collect(),
        });
    }
}

fn write_log(p: &PathBuf, reports: &[Report]) {
    if std::fs::metadata(p).is_ok_and(|m| m.len() > LOG_MAX) {
        let _ = std::fs::rename(p, p.with_extension("log.1"));
    }
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
    else {
        return;
    };
    for r in reports {
        if let Ok(line) = serde_json::to_string(r) {
            let _ = writeln!(f, "{line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MSF and DCF77 keyed as sent, in a bin at 60 kHz whose ADC clock is
    /// 0.5 ppm fast (the carriers appear low), with noise: both decode,
    /// on time, and the carriers give the clock's error.
    /// Two values 5 ms apart, most at one: the scatter shows it.
    #[test]
    fn scatter_sees_two_values() {
        let v: std::collections::VecDeque<f64> =
            [6.2, 6.3, 11.2, 6.2, 6.1, 11.3, 6.2, 6.2, 11.2, 6.3]
                .into_iter()
                .collect();
        let s = scatter(&v);
        assert!(s > 1.5 && s < 3.0, "{s}");
        let steady: std::collections::VecDeque<f64> = std::iter::repeat_n(1206.3, 10).collect();
        assert_eq!(scatter(&steady), SCATTER_MIN_MS);
    }

    #[test]
    fn stations_decode_and_measure_the_clock() {
        let rate = 60_000.0;
        let start: i64 = 1_795_000_020;
        let minutes = 4;
        let ppm = 0.5;
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64 - 0.5
        };
        let st: Vec<(Station, f64, Vec<f32>)> =
            [(Station::Msf, 60_000.0), (Station::Dcf77, 77_500.0)]
                .into_iter()
                .map(|(s, fc)| (s, fc, web888_timecode::keying(s, start, minutes)))
                .collect();
        let mut mon = Monitor::new(&["MSF".into(), "DCF77".into()], 60_000.0, rate).unwrap();
        // at Mainflingen (DCF77's path ~0, MSF's ~1000 km: 3.3 ms; the
        // synthetic signal has none)
        mon.set_receiver((50.0156, 9.0108));
        let utc = |f: f64| Some(start as f64 + f / rate);
        let a = RX_SINE_FULL_SCALE * 1e-3;
        let n = minutes * 60 * rate as usize;
        // runs as small as the unit's (the decoders must not cost by them)
        let block = 128;
        let mut reports = Vec::new();
        let mut buf = Vec::with_capacity(block);
        for b0 in (0..n).step_by(block) {
            buf.clear();
            for k in b0..(b0 + block).min(n) {
                let t = k as f64 / rate;
                let mut z = Complex64::new(rnd() * a * 0.3, rnd() * a * 0.3);
                for (_, fc, key) in &st {
                    let f = fc * (1.0 - ppm * 1e-6) - 60_000.0;
                    let lv = key[(t * 1000.0) as usize] as f64;
                    z += Complex64::from_polar(a * lv, 2.0 * PI * f * t);
                }
                buf.push(Iq32 {
                    i: z.re.round() as i32,
                    q: z.im.round() as i32,
                });
            }
            // the system clock "lately": the ADC's, as `utc` has it
            let fa = (minutes as f64 - 1.0) * 60.0 * rate;
            let recent = |_r: f64| Some((fa, start as f64 + fa / rate));
            reports.extend(mon.push(b0 as u64, &buf, utc, recent, None));
        }
        // the last full minute
        let last: Vec<&Report> = reports
            .iter()
            .filter(|r| r.t == start + 60 * (minutes as i64 - 2))
            .collect();
        assert_eq!(last.len(), 2, "{reports:#?}");
        for r in last {
            assert!(r.ok && r.confirmed, "{r:?}");
            let late = r.late_ms.unwrap();
            // the decoders' own offsets: MSF ~0, DCF77 ~+4.6 ms (its keying)
            assert!(late.abs() < 6.0, "{} {late} ms late", r.station);
            let p = r.adc_ppm.unwrap();
            assert!((p - ppm).abs() < 0.02, "{} {p} ppm", r.station);
            assert!(
                r.carrier_dbfs.unwrap() < -55.0 && r.carrier_dbfs.unwrap() > -70.0,
                "{r:?}"
            );
            assert!(r.snr_db_hz.unwrap() > 20.0, "{r:?}");
            // offset: the calibrated delay less the lateness
            let off = r.offset_ms.unwrap();
            let delay = if r.station == "MSF" {
                path_ms((54.9116, -3.2785), (50.0156, 9.0108)) + decoder_ms(Station::Msf)
            } else {
                decoder_ms(Station::Dcf77)
            };
            assert!((off - (delay - late)).abs() < 0.01, "{r:?}");
            assert!(r.sigma_ms.unwrap() > 0.0);
            assert_eq!(r.at, Some(start as f64 + (minutes as f64 - 1.0) * 60.0));
        }
        assert!(mon.counts.iter().all(|c| c[0] >= 2), "{:?}", mon.counts);
    }

    /// One station keyed as sent, alone in a stream of `rate` S/s centred
    /// on `centre`: the minutes it decodes and confirms.
    fn synth_confirmed(station: Station, fc: f64, centre: f64, rate: f64) -> u64 {
        let start: i64 = 1_795_000_020;
        let minutes = 4;
        let key = web888_timecode::keying(station, start, minutes);
        let mut mon = Monitor::new(&[station.name().into()], centre, rate).unwrap();
        let utc = |f: f64| Some(start as f64 + f / rate);
        let a = RX_SINE_FULL_SCALE * 1e-3;
        let n = minutes * 60 * rate as usize;
        let block = 128;
        let mut buf = Vec::with_capacity(block);
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64 - 0.5
        };
        for b0 in (0..n).step_by(block) {
            buf.clear();
            for k in b0..(b0 + block).min(n) {
                let t = k as f64 / rate;
                let lv = key[(t * 1000.0) as usize] as f64;
                let z = Complex64::new(rnd() * a * 0.3, rnd() * a * 0.3)
                    + Complex64::from_polar(a * lv, 2.0 * PI * (fc - centre) * t);
                buf.push(Iq32 {
                    i: z.re.round() as i32,
                    q: z.im.round() as i32,
                });
            }
            let recent = |_r: f64| Some((0.0, start as f64));
            mon.push(b0 as u64, &buf, utc, recent, None);
        }
        mon.counts[0][0]
    }

    /// The keyed stations decode alone, in a KiwiSDR's 12 kS/s (the
    /// carrier at the centre) as in a channelizer bin; with their keying
    /// as deep as a clean signal gives (the impulse blanker once locked
    /// shut after WWVB's, JJY's and MSF's long lows).
    #[test]
    fn keyed_stations_decode_at_12k() {
        for (st, fc) in [
            (Station::Wwvb, 60_000.0),
            (Station::Jjy, 40_000.0),
            (Station::Jjy, 60_000.0),
            (Station::Dcf77, 77_500.0),
            (Station::Msf, 60_000.0),
            (Station::Bpc, 68_500.0),
        ] {
            let a = synth_confirmed(st, fc, fc, 12_000.0);
            let b = synth_confirmed(st, fc, fc + 10_000.0, 60_000.0);
            assert!(a >= 2 && b >= 2, "{st:?}: {a} at 12 kS/s, {b} at 60 kS/s");
        }
    }
}
