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
/// The carrier over the floor (dB in 1 Hz) for its frequency to count.
const MIN_SNR: f64 = 20.0;

/// One station's minute.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Report {
    /// UTC (s) of the minute's start (the receiver's clock).
    pub t: i64,
    pub station: String,
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
    /// The confirmed minute's start, late by the receiver's clock (ms).
    pub late_ms: Option<f64>,
    /// The ADC clock's error (ppm, + fast) from the carrier's frequency
    /// over the last minute.
    pub adc_ppm: Option<f64>,
    /// The same from the timebase (against the system clock: NTP).
    pub tb_ppm: Option<f64>,
    /// The bin's noise floor over the minute (dBFS/Hz).
    pub floor_dbfs_hz: Option<f64>,
    pub floor_p90_dbfs_hz: Option<f64>,
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
    /// The filter's history, twice (a contiguous window at `pos + 1`).
    hist: Vec<Complex32>,
    pos: usize,
    k: usize,
    /// Decimated samples so far.
    n: u64,
    up: Complex64,
    up_step: Complex64,
    rx: Receiver,
    audio: Vec<f32>,
    /// The current second's sum and count; the seconds' (mid sample, mean).
    sec: (Complex64, usize),
    secs: std::collections::VecDeque<(f64, Complex64)>,
    /// This UTC minute's: the carrier's seconds (amplitude), the minutes
    /// decoded.
    amps: Vec<f64>,
    minutes: Vec<Minute>,
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
            hist: vec![Complex32::default(); 2 * TAPS],
            pos: 0,
            k: 0,
            n: 0,
            up: Complex64::new(1.0, 0.0),
            up_step: Complex64::new(u.cos(), u.sin()),
            rx: Receiver::new(station, FS, TONE, 0.0),
            audio: Vec::with_capacity(CHUNK + 4096),
            sec: (Complex64::default(), 0),
            secs: Default::default(),
            amps: Vec::new(),
            minutes: Vec::new(),
        }
    }

    fn push(&mut self, iq: &[Complex32], decim: usize, taps: &[f32]) {
        for &x in iq {
            let z = if self.table.is_empty() {
                let r = Complex32::new(self.rot.re as f32, self.rot.im as f32);
                self.rot *= self.step;
                x * r
            } else {
                let r = self.table[self.ti];
                self.ti += 1;
                if self.ti == self.table.len() {
                    self.ti = 0;
                }
                x * r
            };
            self.hist[self.pos] = z;
            self.hist[self.pos + TAPS] = z;
            self.pos = (self.pos + 1) % TAPS;
            self.k += 1;
            if self.k < decim {
                continue;
            }
            self.k = 0;
            let w = &self.hist[self.pos..self.pos + TAPS];
            let mut acc = Complex32::default();
            for (h, t) in w.iter().zip(taps) {
                acc += h * t;
            }
            let y = Complex64::new(acc.re as f64, acc.im as f64);
            self.n += 1;
            self.sec.0 += y;
            self.sec.1 += 1;
            if self.sec.1 == FS as usize {
                let mean = self.sec.0 / FS;
                let mid = self.n as f64 - FS / 2.0;
                self.secs.push_back((mid, mean));
                while self.secs.len() > 60 {
                    self.secs.pop_front();
                }
                self.amps.push(mean.norm());
                self.sec = (Complex64::default(), 0);
            }
            self.audio
                .push(((y * self.up).re / RX_SINE_FULL_SCALE) as f32);
            self.up *= self.up_step;
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
    minute: Option<i64>,
    pub counts: Vec<[u64; 3]>,
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
            let fc = st
                .carriers()
                .iter()
                .copied()
                .find(|c| (c - centre).abs() <= CLEAN_HZ)
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
            minute: None,
        })
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
    /// UTC. The minutes that ended, one report per station.
    pub fn push(
        &mut self,
        first: u64,
        iq: &[Iq32],
        utc: impl Fn(f64) -> Option<f64>,
        tb_ppm: Option<f64>,
    ) -> Vec<Report> {
        if self.next != Some(first) {
            // a gap: start again
            for c in &mut self.chains {
                *c = Chain::new(c.station, c.fc, self.centre, self.rate);
            }
            self.frame0 = first;
            self.fbuf.clear();
            self.since_fft = 0;
        }
        self.next = Some(first + iq.len() as u64);
        let z: Vec<Complex32> = iq
            .iter()
            .map(|s| Complex32::new(s.i as f32, s.q as f32))
            .collect();
        // the floor: FFT_N frames at the start of each second
        for &x in &z {
            if self.since_fft < FFT_N as u64 {
                self.fbuf.push(x);
                if self.fbuf.len() == FFT_N {
                    self.floor();
                }
            }
            self.since_fft += 1;
            if self.since_fft >= self.rate as u64 {
                self.since_fft = 0;
            }
        }
        for c in &mut self.chains {
            c.push(&z, self.decim, &self.taps);
        }
        // the minute by the receiver's clock
        let end = first + iq.len() as u64;
        let Some(now) = utc(end as f64) else {
            return vec![];
        };
        let m = (now / 60.0).floor() as i64;
        let prev = self.minute.replace(m);
        match prev {
            Some(p) if p != m => self.reports(p * 60, &utc, tb_ppm),
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
        let k = (20_000.0 / df) as usize;
        // the clean ±20 kHz: bins 0..k and N-k..N
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
        tb_ppm: Option<f64>,
    ) -> Vec<Report> {
        let mut fl = std::mem::take(&mut self.floors);
        fl.sort_by(f64::total_cmp);
        let floor = (!fl.is_empty()).then(|| fl[fl.len() / 2]);
        let p90 = (!fl.is_empty()).then(|| fl[(fl.len() * 9 / 10).min(fl.len() - 1)]);
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
            let late_ms = best.filter(|_| confirmed).and_then(|m| {
                // its first sample's bin frame: the decimated sample, back
                // by the filter's delay
                let f = self.frame0 as f64 + m.t * FS * self.decim as f64 - (TAPS - 1) as f64 / 2.0;
                utc(f).map(|u| ((u - m.utc as f64) * 1e3 * 100.0).round() / 100.0)
            });
            self.counts[i][match (ok, confirmed) {
                (true, true) => 0,
                (true, false) => 1,
                _ => 2,
            }] += 1;
            let r3 = |v: f64| (v * 1e3).round() / 1e3;
            let r1 = |v: f64| (v * 10.0).round() / 10.0;
            out.push(Report {
                t,
                station: c.station.name().into(),
                carrier_dbfs: carrier.map(r1),
                snr_db_hz: carrier.zip(floor).map(|(c, f)| r1(c - f)),
                keying_db: r1(c.rx.level as f64),
                ok,
                confirmed,
                unread: best.map(|m| m.symbols.chars().filter(|&s| s == '?').count()),
                late_ms,
                // a carrier well above the floor only (else its phase is noise)
                adc_ppm: c
                    .ppm()
                    .filter(|_| carrier.zip(floor).is_some_and(|(c, f)| c - f > MIN_SNR))
                    .map(r3),
                tb_ppm: tb_ppm.map(r3),
                floor_dbfs_hz: floor.map(r1),
                floor_p90_dbfs_hz: p90.map(r1),
            });
        }
        out
    }
}

/// Start watching (`[timecode] enabled`): a thread on the bin tap.
pub fn spawn(sh: &Arc<Shared>, cfg: Timecode, state_dir: PathBuf) -> std::io::Result<()> {
    if !cfg.enabled {
        return Ok(());
    }
    let Some(tap) = sh.waterfalls.tap().cloned() else {
        eprintln!("timecode: needs the channelizer's bins; not watching");
        return Ok(());
    };
    let caps = tap.caps();
    let bin = crate::wideiq::nearest_bin(&caps, cfg.centre_hz);
    let centre = caps.freq(bin);
    let mut mon = match Monitor::new(&cfg.stations, centre, caps.frame_rate) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("timecode: {e}; not watching");
            return Ok(());
        }
    };
    eprintln!(
        "timecode: {} in the bin at {:.0} kHz",
        mon.describe(),
        centre / 1e3
    );
    *STATE.lock().unwrap() = Some(State::default());
    let log = cfg.log.then(|| state_dir.join("timecode.log"));
    std::thread::Builder::new()
        .name("timecode".into())
        .spawn(move || run(tap, bin, &mut mon, log))?;
    Ok(())
}

fn run(tap: BinTap, bin: usize, mon: &mut Monitor, log: Option<PathBuf>) {
    let (handle, rx) = tap.subscribe();
    handle.want(vec![bin], 0.0);
    let mut iq: Vec<Iq32> = Vec::new();
    loop {
        let m = match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(m) => m,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => return,
        };
        let TapMsg::Frames(runs) = m else { continue };
        for f in runs.iter() {
            let Some(idx) = f.bins.iter().position(|&b| b == bin) else {
                continue;
            };
            iq.clear();
            iq.extend((0..f.count).map(|k| f.frame(k)[idx]));
            // the lock only for each look-up: the bin reader takes it too
            let ppm = tap.timebase().info().adc_ppm;
            let reports = mon.push(f.first, &iq, |fr| tap.timebase().utc(fr), ppm);
            if reports.is_empty() {
                continue;
            }
            if let Some(p) = &log {
                write_log(p, &reports);
            }
            let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
            *s = Some(State {
                reports,
                counts: mon.counts.clone(),
            });
        }
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
            reports.extend(mon.push(b0 as u64, &buf, utc, None));
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
        }
        assert!(mon.counts.iter().all(|c| c[0] >= 2), "{:?}", mon.counts);
    }
}
