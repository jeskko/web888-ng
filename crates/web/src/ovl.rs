// SPDX-License-Identifier: GPL-3.0-or-later
//! ADC overload events (`doc/m2-web-ui.md` §15): when the ADC overflowed,
//! for how long and how badly, and what did it.
//!
//! The FPGA counts the ADC clocks with the overflow pin high; the hw loop
//! reads the count with every block it reads (a few ms apart) and keeps
//! the blocks that had overflows (`HwStatus::overload_blocks`) and the
//! clocks (`overflow_clocks`). An *event* is a run of such blocks, ended
//! by [`QUIET`] without one (or cut at [`MAX_EVENT`]).
//!
//! With the channelizer (a [`BinTap`]), the monitor also keeps the
//! full-band power lines (2048 bins of 30 kHz, [`POWER_FPS`] a second)
//! and a slow per-bin average of them outside events. An event's lines,
//! held at their highest per bin, say which bins carried the power and
//! how far each rose ([`analyse`]):
//!
//! * **carrier**: one bin holds at least half the power;
//! * **several**: the three strongest hold most of it;
//! * **broadband**: the bins rose together (an impulse: lightning, a
//!   switching spike);
//! * **spread**: none of these (many signals adding up).
//!
//! For a carrier the monitor then reads [`CAPTURE`] samples of its bin
//! (60 kS/s complex, through the tap's burst) and measures the carrier's
//! frequency to a fraction of a hertz ([`carrier`]), if it is still on.
//!
//! Each event goes to the log (stderr and the skimmers' syslog/MQTT log as
//! source "adc"), to the metrics, and to `GET /api/admin/ovl` (the last
//! [`KEEP`]).

use crate::admin::AdminOnly;
use crate::session::Shared;
use axum::Router;
use axum::extract::State;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use rustfft::FftPlanner;
use serde::Serialize;
use serde_json::json;
use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use web888_dsp::C32;
use web888_dsp::binrx::{BinTap, TapHandle, TapMsg};
use web888_dsp::rx_to_c32;
use web888_hw::HwStatus;
use web888_radio::bins::BinCaps;

/// Power lines a second while monitoring (each averages 100 ms).
pub const POWER_FPS: f64 = 10.0;
/// An event ends this long after the last block with an overflow.
const QUIET: Duration = Duration::from_millis(1500);
/// A longer one is reported in pieces.
const MAX_EVENT: Duration = Duration::from_secs(60);
/// Samples of a carrier's bin read for its frequency (0.55 s at 60 kS/s;
/// 1.8 Hz a point before interpolation).
pub const CAPTURE: usize = 32768;
/// How long the samples may take to come.
const CAPTURE_WAIT: Duration = Duration::from_secs(3);
/// Events kept for the admin page.
const KEEP: usize = 100;
/// Power lines kept from before an event.
const HISTORY: Duration = Duration::from_millis(500);

/// One bin of an event's spectrum.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Bin {
    pub bin: usize,
    /// Its centre, and the strongest point's estimate between the bins.
    pub hz: f64,
    pub peak_hz: f64,
    /// Its power during the event (dB, a full-scale sine 0), the rise
    /// over its average outside events, and its share of all the bins'.
    pub dbfs: f64,
    pub rise_db: f64,
    pub share: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Carrier,
    Several,
    Broadband,
    Spread,
    /// No spectrum (no channelizer, or no lines yet).
    Unknown,
}

/// What an event's spectrum says.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Analysis {
    pub kind: Kind,
    /// The strongest bins (at most three), strongest first.
    pub strongest: Vec<Bin>,
    /// The median rise over all bins (dB).
    pub median_rise_db: f64,
    /// The total power (dBFS).
    pub total_dbfs: f64,
}

/// A carrier measured in its bin's samples.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Carrier {
    pub hz: f64,
    /// Its level (dBFS) and how far it stands above the bin's median.
    pub dbfs: f64,
    pub snr_db: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Event {
    /// UTC seconds of the first block with an overflow.
    pub start: f64,
    pub seconds: f64,
    /// Blocks with overflows, and the ADC clocks overflowing.
    pub blocks: u64,
    pub clocks: u64,
    /// The ADC's peak register after the event (dBFS; none: unknown).
    pub peak_dbfs: Option<f64>,
    pub analysis: Analysis,
    pub carrier: Option<Carrier>,
}

fn db(p: f64) -> f64 {
    10.0 * p.max(1e-30).log10()
}

/// The point between bins `k-1`, `k`, `k+1` where a parabola through
/// their levels (dB) peaks: an offset from `k`, -0.5 to 0.5.
pub fn interp(prev: f64, mid: f64, next: f64) -> f64 {
    let d = prev - 2.0 * mid + next;
    if d >= 0.0 || !d.is_finite() {
        return 0.0;
    }
    (0.5 * (prev - next) / d).clamp(-0.5, 0.5)
}

/// An event's spectrum: `lines` (power per bin, a full-scale sine 1)
/// during it, against `avg` (dB per bin) outside events.
pub fn analyse(lines: &[&[f32]], avg: &[f32], caps: &BinCaps) -> Analysis {
    let n = lines
        .iter()
        .map(|l| l.len())
        .min()
        .unwrap_or(0)
        .min(caps.bins);
    if n == 0 {
        return unknown();
    }
    // each bin at its highest during the event
    let hold: Vec<f64> = (0..n)
        .map(|k| lines.iter().map(|l| l[k] as f64).fold(0.0, f64::max))
        .collect();
    let total: f64 = hold.iter().sum();
    let level: Vec<f64> = hold.iter().map(|&p| db(p)).collect();
    let rise = |k: usize| level[k] - avg.get(k).map_or(level[k], |&a| a as f64);
    let mut rises: Vec<f64> = (0..n).map(rise).collect();
    rises.sort_by(f64::total_cmp);
    let median_rise_db = rises[n / 2];
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| hold[b].total_cmp(&hold[a]));
    let strongest: Vec<Bin> = order
        .iter()
        .take(3)
        .map(|&k| {
            let side =
                |j: Option<usize>| j.filter(|&j| j < n).map_or(level[k] - 30.0, |j| level[j]);
            let off = interp(side(k.checked_sub(1)), level[k], side(Some(k + 1)));
            Bin {
                bin: k,
                hz: caps.freq(k),
                peak_hz: caps.freq(k) + off * caps.bin_hz,
                dbfs: level[k],
                rise_db: rise(k),
                share: if total > 0.0 { hold[k] / total } else { 0.0 },
            }
        })
        .collect();
    let top3: f64 = strongest.iter().map(|b| b.share).sum();
    let kind = if strongest[0].share >= 0.5 {
        Kind::Carrier
    } else if median_rise_db >= 3.0 {
        Kind::Broadband
    } else if top3 >= 0.6 {
        Kind::Several
    } else {
        Kind::Spread
    };
    Analysis {
        kind,
        strongest,
        median_rise_db,
        total_dbfs: db(total),
    }
}

/// No spectrum to say anything with.
fn unknown() -> Analysis {
    Analysis {
        kind: Kind::Unknown,
        strongest: Vec::new(),
        median_rise_db: 0.0,
        total_dbfs: f64::NEG_INFINITY,
    }
}

/// The strongest carrier in a bin's samples (`rate` a second, centred on
/// `centre_hz`), within `half_width` Hz of the centre; none when nothing
/// stands 20 dB above the median.
pub fn carrier(x: &[C32], rate: f64, centre_hz: f64, half_width: f64) -> Option<Carrier> {
    let n = x.len();
    if n < 64 || !n.is_power_of_two() {
        return None;
    }
    let mut buf: Vec<C32> = x
        .iter()
        .enumerate()
        .map(|(i, &s)| s * (0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()))
        .collect();
    FftPlanner::<f32>::new()
        .plan_fft_forward(n)
        .process(&mut buf);
    // bins from -n/2 to n/2-1, as dB
    let pw: Vec<f64> = (0..n)
        .map(|i| buf[(i + n / 2) % n].norm_sqr() as f64)
        .map(db)
        .collect();
    let df = rate / n as f64;
    let lim = ((half_width / df) as usize).min(n / 2 - 1);
    let (lo, hi) = (n / 2 - lim, n / 2 + lim);
    let k = (lo..=hi).max_by(|&a, &b| pw[a].total_cmp(&pw[b]))?;
    let mut sorted = pw.clone();
    sorted.sort_by(f64::total_cmp);
    let median = sorted[n / 2];
    if pw[k] - median < 20.0 {
        return None;
    }
    let off = if k > 0 && k + 1 < n {
        interp(pw[k - 1], pw[k], pw[k + 1])
    } else {
        0.0
    };
    // a bin's tone of amplitude A (a full-scale sine 1) peaks at A·n/2
    // through the Hann window
    let amp = buf[(k + n / 2) % n].norm() as f64 * 2.0 / n as f64;
    Some(Carrier {
        hz: centre_hz + (k as f64 - (n / 2) as f64 + off) * df,
        dbfs: db(amp * amp),
        snr_db: pw[k] - median,
    })
}

/// The kept events, newest last.
fn events() -> &'static Mutex<VecDeque<Event>> {
    static E: OnceLock<Mutex<VecDeque<Event>>> = OnceLock::new();
    E.get_or_init(Mutex::default)
}

/// Whether the monitor has the spectrum.
static SPECTRUM: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

fn mhz(hz: f64) -> String {
    format!("{:.3} MHz", hz / 1e6)
}

/// The event in a line for the log.
pub fn describe(e: &Event) -> String {
    let t = web888_skim::utc::utc(e.start);
    let mut s = format!(
        "ADC overload at {:02}:{:02}:{:02} UTC, {:.1} s, {} clocks over in {} blocks",
        t.hour, t.min, t.sec, e.seconds, e.clocks, e.blocks
    );
    if let Some(p) = e.peak_dbfs {
        s += &format!(", peak {p:.1} dBFS");
    }
    let a = &e.analysis;
    let top = a.strongest.first();
    match (a.kind, top) {
        (Kind::Unknown, _) | (_, None) => {}
        (Kind::Carrier, Some(b)) => {
            s += &format!(
                ": a carrier near {} ({:.0} dBFS, {:.0} % of the power, +{:.0} dB)",
                mhz(b.peak_hz),
                b.dbfs,
                100.0 * b.share,
                b.rise_db
            );
        }
        (Kind::Several, _) => {
            let list: Vec<String> = a
                .strongest
                .iter()
                .map(|b| format!("{} ({:.0} dBFS)", mhz(b.peak_hz), b.dbfs))
                .collect();
            s += &format!(": several signals, strongest {}", list.join(", "));
        }
        (Kind::Broadband, _) => {
            s += &format!(
                ": broadband, the whole band up {:.0} dB (an impulse)",
                a.median_rise_db
            );
        }
        (Kind::Spread, Some(b)) => {
            s += &format!(
                ": no single cause, strongest {} ({:.0} dBFS, {:.0} %)",
                mhz(b.peak_hz),
                b.dbfs,
                100.0 * b.share
            );
        }
    }
    if let Some(c) = &e.carrier {
        s += &format!("; carrier measured at {:.1} Hz, {:.0} dBFS", c.hz, c.dbfs);
    }
    s
}

/// A carrier's bin being read.
struct Capture {
    bin: usize,
    centre: f64,
    since: Instant,
    next: Option<u64>,
    x: Vec<C32>,
}

impl Capture {
    fn take(&mut self, f: &web888_radio::bins::BinFrames) {
        let Ok(j) = f.bins.binary_search(&self.bin) else {
            return;
        };
        if self.next.is_some_and(|n| n != f.first) {
            // a gap: start again
            self.x.clear();
        }
        let n = f.bins.len();
        for k in 0..f.count {
            if self.x.len() < CAPTURE {
                self.x.push(rx_to_c32(f.iq[k * n + j]));
            }
        }
        self.next = Some(f.end());
    }
}

struct Open {
    start: f64,
    began: Instant,
    last: Instant,
    blocks0: u64,
    clocks0: u64,
    lines: Vec<Arc<Vec<f32>>>,
    analysed: Option<Analysis>,
    capture: Option<Capture>,
    carrier: Option<Carrier>,
}

/// The monitor's state; [`Monitor::tick`] every 50 ms.
struct Monitor {
    status: Arc<HwStatus>,
    caps: Option<BinCaps>,
    tap: Option<(TapHandle, Receiver<TapMsg>)>,
    /// Recent lines (arrival, line) and the per-bin average outside events.
    recent: VecDeque<(Instant, Arc<Vec<f32>>)>,
    avg: Vec<f32>,
    blocks: u64,
    clocks: u64,
    open: Option<Open>,
    calm_since: Instant,
}

impl Monitor {
    fn new(status: Arc<HwStatus>, tap: Option<BinTap>) -> Self {
        let caps = tap.as_ref().map(|t| t.caps());
        let tap = tap.map(|t| {
            let (h, rx) = t.subscribe();
            h.want(Vec::new(), POWER_FPS);
            (h, rx)
        });
        SPECTRUM.store(tap.is_some(), Ordering::Relaxed);
        let blocks = status.overload_blocks.load(Ordering::Relaxed);
        let clocks = status.overflow_clocks.load(Ordering::Relaxed);
        Monitor {
            status,
            caps,
            tap,
            recent: VecDeque::new(),
            avg: Vec::new(),
            blocks,
            clocks,
            open: None,
            calm_since: Instant::now(),
        }
    }

    fn input(&mut self, now: Instant) {
        let Some((_, rx)) = &self.tap else { return };
        while let Ok(m) = rx.try_recv() {
            match m {
                TapMsg::Power(line) => {
                    if let Some(o) = &mut self.open {
                        o.lines.push(line.clone());
                    } else if now.duration_since(self.calm_since) > Duration::from_secs(2) {
                        // the average outside events (~10 s)
                        if self.avg.len() != line.len() {
                            self.avg = line.iter().map(|&p| db(p as f64) as f32).collect();
                        } else {
                            for (a, &p) in self.avg.iter_mut().zip(line.iter()) {
                                *a += 0.01 * (db(p as f64) as f32 - *a);
                            }
                        }
                    }
                    self.recent.push_back((now, line));
                    while self
                        .recent
                        .front()
                        .is_some_and(|(t, _)| now.duration_since(*t) > HISTORY)
                    {
                        self.recent.pop_front();
                    }
                }
                TapMsg::Frames(fr) => {
                    if let Some(c) = self.open.as_mut().and_then(|o| o.capture.as_mut()) {
                        for f in fr.iter() {
                            c.take(f);
                        }
                    }
                }
                TapMsg::Zoom(_) => {}
            }
        }
    }

    /// The event's spectrum so far.
    fn analyse(&self, o: &Open) -> Analysis {
        let lines: Vec<&[f32]> = o.lines.iter().map(|l| &l[..]).collect();
        match &self.caps {
            Some(c) => analyse(&lines, &self.avg, c),
            None => unknown(),
        }
    }

    fn tick(&mut self, now: Instant) -> Option<Event> {
        self.input(now);
        let blocks = self.status.overload_blocks.load(Ordering::Relaxed);
        let clocks = self.status.overflow_clocks.load(Ordering::Relaxed);
        let more = blocks != self.blocks;
        if more {
            self.calm_since = now;
            match &mut self.open {
                Some(o) => o.last = now,
                None => {
                    self.open = Some(Open {
                        start: unix_now(),
                        began: now,
                        last: now,
                        blocks0: self.blocks,
                        clocks0: self.clocks,
                        // the lines just before: the event began in one of them
                        lines: self.recent.iter().map(|(_, l)| l.clone()).collect(),
                        analysed: None,
                        capture: None,
                        carrier: None,
                    })
                }
            }
        }
        self.blocks = blocks;
        self.clocks = clocks;
        let caps = self.caps;
        let Some(o) = &mut self.open else { return None };
        // a first look 0.35 s in: a carrier's bin is read while it is on
        if o.analysed.is_none() && now.duration_since(o.began) >= Duration::from_millis(350) {
            let lines: Vec<&[f32]> = o.lines.iter().map(|l| &l[..]).collect();
            if let Some(c) = &caps {
                let a = analyse(&lines, &self.avg, c);
                if a.kind == Kind::Carrier
                    && let (Some(b), Some((h, _))) = (a.strongest.first(), &self.tap)
                {
                    let frames = CAPTURE + CAPTURE / 2;
                    h.burst(vec![b.bin], frames, true);
                    o.capture = Some(Capture {
                        bin: b.bin,
                        centre: b.hz,
                        since: now,
                        next: None,
                        x: Vec::with_capacity(CAPTURE),
                    });
                }
                o.analysed = Some(a);
            } else {
                o.analysed = Some(unknown());
            }
        }
        if let Some(c) = &o.capture
            && (c.x.len() >= CAPTURE || now.duration_since(c.since) > CAPTURE_WAIT)
        {
            let c = o.capture.take().expect("some");
            if c.x.len() >= CAPTURE
                && let Some(caps) = &caps
            {
                o.carrier = carrier(&c.x, caps.frame_rate, c.centre, caps.bin_hz * 0.6);
            }
            if let Some((h, _)) = &self.tap {
                h.burst(Vec::new(), 0, true);
            }
        }
        let ended = now.duration_since(o.last) >= QUIET && o.capture.is_none();
        let cut = now.duration_since(o.began) >= MAX_EVENT;
        if !(ended || cut) {
            return None;
        }
        let o = self.open.take().expect("some");
        let analysis = self.analyse(&o);
        let peak = self.status.adc_peak.load(Ordering::Relaxed);
        Some(Event {
            start: o.start,
            seconds: o.last.duration_since(o.began).as_secs_f64(),
            blocks: blocks - o.blocks0,
            clocks: clocks - o.clocks0,
            peak_dbfs: (peak > 0).then(|| 20.0 * (peak as f64 / 32768.0).log10()),
            analysis,
            carrier: o.carrier,
        })
    }
}

/// Watches for overloads (a thread).
pub fn spawn(status: Arc<HwStatus>, tap: Option<BinTap>) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("ovl".into())
        .spawn(move || {
            let mut m = Monitor::new(status, tap);
            loop {
                std::thread::sleep(Duration::from_millis(50));
                if let Some(e) = m.tick(Instant::now()) {
                    web888_skim::events::emit(
                        web888_skim::events::Level::Warning,
                        "adc",
                        describe(&e),
                    );
                    let mut l = events().lock().unwrap_or_else(|e| e.into_inner());
                    if l.len() == KEEP {
                        l.pop_front();
                    }
                    l.push_back(e);
                }
            }
        })?;
    Ok(())
}

pub fn routes() -> Router<Arc<Shared>> {
    Router::new().route("/api/admin/ovl", get(list))
}

async fn list(_: AdminOnly, State(sh): State<Arc<Shared>>) -> Response {
    let l: Vec<Event> = events()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .rev()
        .cloned()
        .collect();
    let peak = sh.status.adc_peak.load(Ordering::Relaxed);
    Json(json!({
        "spectrum": SPECTRUM.load(Ordering::Relaxed),
        "blocks": sh.status.overload_blocks.load(Ordering::Relaxed),
        "clocks": sh.status.overflow_clocks.load(Ordering::Relaxed),
        "peak_dbfs": (peak > 0).then(|| 20.0 * (peak as f64 / 32768.0).log10()),
        "events": l,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAPS: BinCaps = BinCaps {
        bin_hz: 30_000.0,
        frame_rate: 60_000.0,
        bins: 2048,
        first_hz: 0.0,
        wraps: false,
        max_selected: 64,
    };

    /// A quiet band at -100 dBFS a bin (avg the same), with `add` on top.
    fn line(add: &[(usize, f32)]) -> Vec<f32> {
        let mut l = vec![1e-10f32; 2048];
        for &(k, p) in add {
            l[k] += p;
        }
        l
    }

    #[test]
    fn a_carrier_several_signals_an_impulse() {
        let avg = vec![-100.0f32; 2048];
        // a carrier at 7.2 MHz (bin 240), between it and bin 241
        let l = line(&[(240, 0.5), (241, 0.25), (239, 0.01)]);
        let a = analyse(&[&l], &avg, &CAPS);
        assert_eq!(a.kind, Kind::Carrier);
        let b = &a.strongest[0];
        assert_eq!(b.bin, 240);
        assert!((b.dbfs + 3.0).abs() < 0.1);
        assert!((b.rise_db - 97.0).abs() < 0.2);
        assert!(b.share > 0.6);
        assert!(
            b.peak_hz > 7_200_000.0 && b.peak_hz < 7_215_000.0,
            "{}",
            b.peak_hz
        );
        // three of equal strength
        let l = line(&[(100, 0.2), (400, 0.2), (700, 0.2)]);
        assert_eq!(analyse(&[&l], &avg, &CAPS).kind, Kind::Several);
        // every bin up 10 dB
        let l = vec![1e-9f32; 2048];
        let a = analyse(&[&l], &avg, &CAPS);
        assert_eq!(a.kind, Kind::Broadband);
        assert!((a.median_rise_db - 10.0).abs() < 0.1);
        // many signals, none ahead: spread
        let many: Vec<(usize, f32)> = (0..40).map(|i| (i * 50, 0.01)).collect();
        assert_eq!(analyse(&[&line(&many)], &avg, &CAPS).kind, Kind::Spread);
        // a line before the carrier and one with it: held at the highest
        let quiet = line(&[]);
        let loud = line(&[(240, 0.5)]);
        assert_eq!(analyse(&[&quiet, &loud], &avg, &CAPS).strongest[0].bin, 240);
        // no lines: unknown
        assert_eq!(analyse(&[], &avg, &CAPS).kind, Kind::Unknown);
    }

    #[test]
    fn a_carriers_frequency_from_its_bins_samples() {
        // 7 203 456.7 Hz in bin 240 (7.2 MHz): 3456.7 Hz from its centre,
        // amplitude 0.5 (-6 dBFS), with a little noise
        let (rate, n) = (60_000.0f64, CAPTURE);
        let f = 3456.7;
        let mut seed = 1u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        };
        let x: Vec<C32> = (0..n)
            .map(|i| {
                let p = std::f64::consts::TAU * f * i as f64 / rate;
                C32::new(
                    0.5 * p.cos() as f32 + 1e-3 * noise(),
                    0.5 * p.sin() as f32 + 1e-3 * noise(),
                )
            })
            .collect();
        let c = carrier(&x, rate, 7_200_000.0, 18_000.0).unwrap();
        assert!((c.hz - 7_203_456.7).abs() < 0.2, "{}", c.hz);
        assert!((c.dbfs + 6.0).abs() < 1.5, "{}", c.dbfs);
        assert!(c.snr_db > 40.0);
        // below the centre too
        let x: Vec<C32> = x.iter().map(|s| s.conj()).collect();
        let c = carrier(&x, rate, 7_200_000.0, 18_000.0).unwrap();
        assert!((c.hz - 7_196_543.3).abs() < 0.2, "{}", c.hz);
        // noise only: none
        let x: Vec<C32> = (0..n).map(|_| C32::new(noise(), noise())).collect();
        assert_eq!(carrier(&x, rate, 0.0, 18_000.0), None);
    }

    #[test]
    fn interpolation_between_bins() {
        assert_eq!(interp(-10.0, 0.0, -10.0), 0.0);
        assert!(interp(-3.0, 0.0, -20.0) < 0.0);
        assert!(interp(-20.0, 0.0, -3.0) > 0.0);
        assert_eq!(interp(0.0, 0.0, 0.0), 0.0);
    }

    #[test]
    fn events_are_counted_and_described() {
        let st = Arc::new(HwStatus::default());
        let mut m = Monitor::new(st.clone(), None);
        let t0 = Instant::now();
        assert!(m.tick(t0).is_none());
        st.overload_blocks.store(3, Ordering::Relaxed);
        st.overflow_clocks.store(1234, Ordering::Relaxed);
        assert!(m.tick(t0 + Duration::from_millis(50)).is_none());
        st.overload_blocks.store(5, Ordering::Relaxed);
        st.overflow_clocks.store(2000, Ordering::Relaxed);
        assert!(m.tick(t0 + Duration::from_millis(450)).is_none());
        // quiet for 1.5 s: the event
        let e = m.tick(t0 + Duration::from_millis(2000)).unwrap();
        assert_eq!((e.blocks, e.clocks), (5, 2000));
        assert!((e.seconds - 0.4).abs() < 1e-6);
        assert_eq!(e.analysis.kind, Kind::Unknown);
        assert!(
            describe(&e).contains("0.4 s, 2000 clocks over in 5 blocks"),
            "{}",
            describe(&e)
        );
        assert!(m.tick(t0 + Duration::from_millis(2100)).is_none());
    }
}
