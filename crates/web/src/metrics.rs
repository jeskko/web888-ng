// SPDX-License-Identifier: GPL-3.0-or-later
//! `GET /metrics`: the server's state in Prometheus' text format (when
//! `web.metrics` is on). Everything here is a gauge or a counter read at
//! the scrape; the history is the scraper's.
//!
//! What it covers: the build and the radio; sessions by protocol and the
//! channels they lease by class; the DSP pool per worker and per kind of
//! job; waterfall views; the skimmers per band; the uploaders, gpsd and
//! the log sinks; the system (CPU, temperature, memory, file systems) and
//! CPU per thread.

use crate::session::Shared;
use std::collections::BTreeMap;
use std::fmt::Write;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::SystemTime;
use web888_core::leases::Class;
use web888_core::pool::JobKind;

/// Frames a protocol's sessions dropped (their client too slow).
#[derive(Default)]
pub struct Drops {
    pub audio: AtomicU64,
    pub wf: AtomicU64,
}

impl Drops {
    const fn new() -> Self {
        Drops {
            audio: AtomicU64::new(0),
            wf: AtomicU64::new(0),
        }
    }
}

pub static WEB: Drops = Drops::new();
pub static KIWI: Drops = Drops::new();

/// Kiwi sessions open (the rest of [`Shared::sessions`] are web ones).
pub static KIWI_SESSIONS: AtomicUsize = AtomicUsize::new(0);

static INFO: OnceLock<Vec<(String, String)>> = OnceLock::new();

/// More labels for `build_info` (what the radio's bitstream is), set once
/// at start.
pub fn set_info(labels: Vec<(String, String)>) {
    let _ = INFO.set(labels);
}

/// A label value in Prometheus' escaping.
fn esc(v: &str) -> String {
    v.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// `{a="x",b="y"}` (nothing for no labels).
fn labels(l: &[(&str, &str)]) -> String {
    if l.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = l
        .iter()
        .map(|(k, v)| format!("{k}=\"{}\"", esc(v)))
        .collect();
    format!("{{{}}}", parts.join(","))
}

struct Out(String);

impl Out {
    fn put(&mut self, name: &str, kind: &str, help: &str, rows: &[(String, f64)]) {
        if rows.is_empty() {
            return;
        }
        let _ = writeln!(
            self.0,
            "# HELP web888d_{name} {help}\n# TYPE web888d_{name} {kind}"
        );
        for (l, v) in rows {
            let _ = writeln!(self.0, "web888d_{name}{l} {v}");
        }
    }

    fn one(&mut self, name: &str, kind: &str, help: &str, v: f64) {
        self.put(name, kind, help, &[(String::new(), v)]);
    }
}

fn n(a: &AtomicU64) -> f64 {
    a.load(Ordering::Relaxed) as f64
}

fn flag(b: bool) -> f64 {
    b as u8 as f64
}

fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// The scrape.
pub fn render(sh: &Shared) -> String {
    let mut o = Out(String::new());
    build(&mut o, sh);
    clients(&mut o, sh);
    hardware(&mut o, sh);
    pool(&mut o, sh);
    waterfalls(&mut o, sh);
    skimmers(&mut o, sh);
    uploaders(&mut o, sh);
    system(&mut o);
    o.0
}

fn build(o: &mut Out, sh: &Shared) {
    let r = &sh.radio;
    let mut l: Vec<(&str, &str)> = vec![
        ("version", env!("CARGO_PKG_VERSION")),
        ("backend", &r.backend),
        ("variant", &r.variant),
        (
            "rx",
            if sh.waterfalls.bins_mode() {
                "bins"
            } else {
                "ddc"
            },
        ),
    ];
    if let Some(info) = INFO.get() {
        l.extend(info.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }
    o.put(
        "build_info",
        "gauge",
        "The server's version, the radio's backend and variant (hf, vhf), and how its RX channels are made (ddc: the hardware's DDCs, bins: a channelizer's).",
        &[(labels(&l), 1.0)],
    );
    let started = unix_now() - sh.kiwi.started.elapsed().as_secs_f64();
    o.one(
        "start_time_seconds",
        "gauge",
        "When the server started (Unix time).",
        started,
    );
    o.one("adc_hz", "gauge", "The ADC's sample rate.", r.adc_hz);
}

fn clients(o: &mut Out, sh: &Shared) {
    let all = sh.sessions.load(Ordering::Relaxed);
    let kiwi = KIWI_SESSIONS.load(Ordering::Relaxed).min(all);
    o.put(
        "sessions",
        "gauge",
        "Listener sessions by protocol (web: this server's page; kiwi: Kiwi clients). HPSDR clients are in channels_in_use{class=\"protocol\"}.",
        &[
            (labels(&[("protocol", "web")]), (all - kiwi) as f64),
            (labels(&[("protocol", "kiwi")]), kiwi as f64),
        ],
    );
    let leases = sh.channels.leases();
    let by = |granted: bool| -> Vec<(String, f64)> {
        Class::ALL
            .iter()
            .map(|c| {
                let k = leases
                    .iter()
                    .filter(|l| l.class == *c && l.state.channel().is_some() == granted)
                    .count();
                (labels(&[("class", c.name())]), k as f64)
            })
            .collect()
    };
    o.one(
        "channels",
        "gauge",
        "RX channels.",
        sh.channels.channels() as f64,
    );
    o.put(
        "channels_in_use",
        "gauge",
        "RX channels leased, by class (web and kiwi listeners are web; HPSDR is protocol).",
        &by(true),
    );
    o.put(
        "lease_queue",
        "gauge",
        "Lease requests waiting for a channel, by class.",
        &by(false),
    );
    let drops = |f: &dyn Fn(&Drops) -> &AtomicU64| -> Vec<(String, f64)> {
        [("web", &WEB), ("kiwi", &KIWI)]
            .iter()
            .map(|(p, d)| (labels(&[("protocol", p)]), n(f(d))))
            .collect()
    };
    o.put(
        "audio_frames_dropped_total",
        "counter",
        "Audio frames dropped: the client didn't keep up.",
        &drops(&|d| &d.audio),
    );
    o.put(
        "wf_frames_dropped_total",
        "counter",
        "Waterfall lines dropped: the client didn't keep up.",
        &drops(&|d| &d.wf),
    );
}

fn hardware(o: &mut Out, sh: &Shared) {
    let s = &sh.status;
    o.one(
        "adc_overloads_total",
        "counter",
        "Blocks with an ADC overload.",
        n(&s.overload_blocks),
    );
    o.one(
        "rx_frames_total",
        "counter",
        "RX frames read from the hardware.",
        n(&s.rx_frames),
    );
    o.one(
        "clock_ppm",
        "gauge",
        "ADC clock error (GPS PPS; 0 without one).",
        s.clock_ppb.load(Ordering::Relaxed) as f64 / 1e3,
    );
    o.one(
        "hw_rate_ok",
        "gauge",
        "The hardware keeps up with the RX rate.",
        flag(s.rx_rate_ok.load(Ordering::Relaxed)),
    );
}

fn pool(o: &mut Out, sh: &Shared) {
    let pool = sh.pool.stats();
    let per = |f: &dyn Fn(&web888_core::pool::WorkerStats) -> f64| -> Vec<(String, f64)> {
        pool.iter()
            .enumerate()
            .map(|(k, w)| (labels(&[("worker", &k.to_string())]), f(w)))
            .collect()
    };
    o.put(
        "pool_busy_seconds_total",
        "counter",
        "DSP worker busy time.",
        &per(&|w| w.busy.as_secs_f64()),
    );
    o.put(
        "pool_shed_level",
        "gauge",
        "DSP worker load shedding level.",
        &per(&|w| w.shed as f64),
    );
    o.put(
        "pool_backlog_blocks",
        "gauge",
        "Most RX blocks a DSP worker had waiting in the last second.",
        &per(&|w| w.backlog as f64),
    );
    o.put(
        "pool_rx_dropped_total",
        "counter",
        "RX blocks a DSP worker lost.",
        &per(&|w| w.rx_dropped as f64),
    );
    o.put(
        "pool_wf_dropped_total",
        "counter",
        "WF blocks a DSP worker lost.",
        &per(&|w| w.wf_dropped as f64),
    );
    let per_kind = |f: &dyn Fn(&(usize, std::time::Duration)) -> f64| -> Vec<(String, f64)> {
        let mut rows = vec![];
        for (k, w) in pool.iter().enumerate() {
            for (i, kind) in JobKind::ALL.iter().enumerate() {
                rows.push((
                    labels(&[("worker", &k.to_string()), ("kind", kind.name())]),
                    f(&w.kinds[i]),
                ));
            }
        }
        rows
    };
    o.put(
        "pool_jobs",
        "gauge",
        "Jobs on a DSP worker, by kind (audio: a listener's audio; waterfall, bins: a waterfall view from the WF FFT or the channelizer's bins; skimmer: a skimmer band's audio).",
        &per_kind(&|k| k.0 as f64),
    );
    o.put(
        "pool_job_seconds_total",
        "counter",
        "DSP worker time by kind of job (estimated from every 8th round; the sum is close to pool_busy_seconds_total).",
        &per_kind(&|k| k.1.as_secs_f64()),
    );
}

fn waterfalls(o: &mut Out, sh: &Shared) {
    let views = sh.waterfalls.views();
    // by span (the decimation), a handful of values
    let mut by: BTreeMap<u16, (f64, f64, f64)> = BTreeMap::new();
    for v in &views {
        let e = by.entry(v.decim).or_default();
        e.0 += 1.0;
        e.1 += v.sessions.len() as f64;
        e.2 += v.fps;
    }
    let rows = |f: &dyn Fn(&(f64, f64, f64)) -> f64| -> Vec<(String, f64)> {
        by.iter()
            .map(|(d, e)| (labels(&[("decim", &d.to_string())]), f(e)))
            .collect()
    };
    o.one(
        "wf_views_open",
        "gauge",
        "Waterfall views open (sessions with the same view share one).",
        views.len() as f64,
    );
    o.put(
        "wf_views",
        "gauge",
        "Waterfall views by span (decim: the ADC rate over the span's).",
        &rows(&|e| e.0),
    );
    o.put(
        "wf_view_sessions",
        "gauge",
        "Sessions watching waterfall views, by span.",
        &rows(&|e| e.1),
    );
    o.put(
        "wf_view_fps",
        "gauge",
        "Waterfall lines per second drawn, by span.",
        &rows(&|e| e.2),
    );
}

fn skimmers(o: &mut Out, sh: &Shared) {
    let Some(sk) = sh.skim.as_ref().and_then(|k| k.skimmers.as_ref()) else {
        return;
    };
    let bands = sk.bands();
    let per = |f: &dyn Fn(&web888_skim::BandStatus) -> f64| -> Vec<(String, f64)> {
        bands
            .iter()
            .map(|b| {
                let mode = format!("{:?}", b.cfg.mode).to_lowercase();
                let dial = format!("{:.0}", b.cfg.dial);
                let l = labels(&[
                    ("mode", &mode),
                    ("dial", &dial),
                    ("band", web888_skim::spot::band_name(b.cfg.dial)),
                ]);
                (l, f(&b.status))
            })
            .collect()
    };
    o.put(
        "skim_slots_total",
        "counter",
        "Slots a skimmer band decoded.",
        &per(&|s| n(&s.slots)),
    );
    o.put(
        "skim_spots_total",
        "counter",
        "Spots a skimmer band decoded.",
        &per(&|s| n(&s.spots)),
    );
    o.put(
        "skim_slots_dropped_total",
        "counter",
        "Slots dropped: the decoders were behind.",
        &per(&|s| n(&s.dropped)),
    );
    o.put(
        "skim_slots_cut_total",
        "counter",
        "Slots whose decoding the deadline cut short.",
        &per(&|s| n(&s.cut_short)),
    );
    o.put(
        "skim_slots_shed_total",
        "counter",
        "Slots not decoded: the band was shed (the decoders short of time).",
        &per(&|s| n(&s.shed_slots)),
    );
    o.put(
        "skim_shed",
        "gauge",
        "The band is shed for now (1).",
        &per(&|s| flag(s.shed.load(Ordering::Relaxed))),
    );
    o.put(
        "skim_granted",
        "gauge",
        "The band has an RX channel (1; 0: waiting for one).",
        &per(&|s| flag(s.granted.load(Ordering::Relaxed))),
    );
    o.put(
        "skim_cpu_seconds_total",
        "counter",
        "Decoder CPU time of a skimmer band.",
        &per(&|s| n(&s.cpu_us) / 1e6),
    );
}

fn uploaders(o: &mut Out, sh: &Shared) {
    let skim = sh.skim.as_ref();
    if let Some(p) = skim.and_then(|k| k.pskreporter.as_ref()) {
        let p = p.lock().unwrap().clone();
        o.one(
            "pskreporter_enabled",
            "gauge",
            "Uploading to PSKReporter (on, with a callsign and a locator).",
            flag(p.enabled),
        );
        o.one(
            "pskreporter_reports_total",
            "counter",
            "Spots sent to PSKReporter.",
            p.reports as f64,
        );
        o.one(
            "pskreporter_datagrams_total",
            "counter",
            "Datagrams sent to PSKReporter.",
            p.datagrams as f64,
        );
        o.put(
            "pskreporter_skipped_total",
            "counter",
            "Spots not sent (not_live: the radio isn't on the air; recent: the station was reported on the band lately).",
            &[
                (
                    labels(&[("reason", "not_live")]),
                    p.skipped_not_live as f64,
                ),
                (labels(&[("reason", "recent")]), p.skipped_recent as f64),
            ],
        );
        o.one(
            "pskreporter_pending",
            "gauge",
            "Spots waiting for the next datagram.",
            p.pending as f64,
        );
        if let Some(t) = p.last_sent {
            o.one(
                "pskreporter_last_sent_seconds",
                "gauge",
                "When the last datagram went (Unix time).",
                t as f64,
            );
        }
    }
    if let Some(w) = skim.and_then(|k| k.wsprnet.as_ref()) {
        let w = w.lock().unwrap().clone();
        o.one(
            "wsprnet_enabled",
            "gauge",
            "Uploading to WSPRnet.",
            flag(w.enabled),
        );
        o.one(
            "wsprnet_spots_total",
            "counter",
            "WSPR spots sent to WSPRnet.",
            w.sent as f64,
        );
        o.one(
            "wsprnet_failed_total",
            "counter",
            "WSPR spots WSPRnet didn't take (given up on).",
            w.failed as f64,
        );
        o.one(
            "wsprnet_skipped_total",
            "counter",
            "WSPR spots not sent: the radio isn't on the air.",
            w.skipped_not_live as f64,
        );
        o.one(
            "wsprnet_pending",
            "gauge",
            "WSPR spots waiting to be sent.",
            w.pending as f64,
        );
        o.one(
            "wsprnet_error",
            "gauge",
            "The last upload failed (1).",
            flag(w.last_error.is_some()),
        );
        if let Some(t) = w.last_sent {
            o.one(
                "wsprnet_last_sent_seconds",
                "gauge",
                "When the last spot went (Unix time).",
                t as f64,
            );
        }
    }
    if let Some(g) = skim.and_then(|k| k.gpsd.as_ref()) {
        let g = g.lock().unwrap().clone();
        o.one(
            "gpsd_connected",
            "gauge",
            "Connected to gpsd.",
            flag(g.connected),
        );
        o.one(
            "gpsd_fix_mode",
            "gauge",
            "gpsd's fix: 0 or 1 none, 2 2D, 3 3D.",
            g.mode as f64,
        );
        if let Some(t) = g.last_fix {
            o.one(
                "gpsd_last_fix_seconds",
                "gauge",
                "When the last fix came (Unix time).",
                t,
            );
        }
    }
    let sinks = [
        ("mqtt", &web888_skim::sink::MQTT),
        ("syslog", &web888_skim::sink::SYSLOG),
    ];
    let rows = |f: &dyn Fn(&web888_skim::sink::SinkStats) -> f64| -> Vec<(String, f64)> {
        sinks
            .iter()
            .map(|(name, s)| (labels(&[("sink", name)]), f(s)))
            .collect()
    };
    let b = |a: &std::sync::atomic::AtomicBool| flag(a.load(Ordering::Relaxed));
    o.put(
        "log_sink_configured",
        "gauge",
        "A log sink has a server set.",
        &rows(&|s| b(&s.configured)),
    );
    o.put(
        "log_sink_connected",
        "gauge",
        "A log sink is connected (syslog over UDP: its socket is open).",
        &rows(&|s| b(&s.connected)),
    );
    o.put(
        "log_sink_messages_total",
        "counter",
        "Messages a log sink sent.",
        &rows(&|s| n(&s.sent)),
    );
    o.put(
        "log_sink_dropped_total",
        "counter",
        "Messages a log sink dropped (not connected, or the send failed).",
        &rows(&|s| n(&s.dropped)),
    );
    o.put(
        "log_sink_errors_total",
        "counter",
        "A log sink's failed connects and sends.",
        &rows(&|s| n(&s.errors)),
    );
}

fn system(o: &mut Out) {
    let hz = crate::admin::clock_ticks();
    if let Some((busy, total)) = crate::sysstat::cpu_jiffies() {
        o.put(
            "system_cpu_seconds_total",
            "counter",
            "CPU time of all CPUs, busy or idle (with iowait).",
            &[
                (labels(&[("mode", "busy")]), busy as f64 / hz),
                (labels(&[("mode", "idle")]), (total - busy) as f64 / hz),
            ],
        );
    }
    if let Some(t) = crate::sysstat::temperature() {
        o.one(
            "temperature_celsius",
            "gauge",
            "The SoC's temperature.",
            t as f64,
        );
    }
    if let Ok(s) = std::fs::read_to_string("/proc/loadavg") {
        let v: Vec<f64> = s
            .split_whitespace()
            .take(3)
            .filter_map(|x| x.parse().ok())
            .collect();
        if v.len() == 3 {
            o.put(
                "load_average",
                "gauge",
                "The system's load average.",
                &[
                    (labels(&[("period", "1m")]), v[0]),
                    (labels(&[("period", "5m")]), v[1]),
                    (labels(&[("period", "15m")]), v[2]),
                ],
            );
        }
    }
    if let Ok(s) = std::fs::read_to_string("/proc/meminfo") {
        let rows: Vec<(String, f64)> = meminfo(&s)
            .into_iter()
            .map(|(k, v)| (labels(&[("kind", k)]), v))
            .collect();
        o.put(
            "memory_bytes",
            "gauge",
            "The system's memory: total, available (what can be had without swapping).",
            &rows,
        );
    }
    if let Some(rss) = resident() {
        o.one(
            "process_resident_bytes",
            "gauge",
            "The server's resident memory.",
            rss,
        );
    }
    let mut size = vec![];
    let mut avail = vec![];
    for p in ["/", "/tmp", "/data"] {
        if let Some((s, a)) = statvfs(p) {
            size.push((labels(&[("path", p)]), s));
            avail.push((labels(&[("path", p)]), a));
        }
    }
    o.put(
        "filesystem_size_bytes",
        "gauge",
        "Size of the file system at a path (/tmp is in RAM on the Web-888).",
        &size,
    );
    o.put(
        "filesystem_avail_bytes",
        "gauge",
        "Space free on the file system at a path.",
        &avail,
    );
    let mut by_name: BTreeMap<String, u64> = BTreeMap::new();
    for (_, name, ticks) in crate::admin::threads() {
        *by_name.entry(name).or_default() += ticks;
    }
    let rows: Vec<(String, f64)> = by_name
        .into_iter()
        .map(|(name, t)| (labels(&[("thread", &name)]), t as f64 / hz))
        .collect();
    o.put(
        "thread_cpu_seconds_total",
        "counter",
        "CPU time per thread name.",
        &rows,
    );
}

/// MemTotal and MemAvailable, bytes.
fn meminfo(s: &str) -> Vec<(&'static str, f64)> {
    let kb = |key: &str| {
        s.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix(':'))
            .and_then(|r| r.split_whitespace().next()?.parse::<f64>().ok())
            .map(|v| v * 1024.0)
    };
    [("total", "MemTotal"), ("available", "MemAvailable")]
        .into_iter()
        .filter_map(|(k, key)| Some((k, kb(key)?)))
        .collect()
}

fn page_size() -> f64 {
    // SAFETY: sysconf has no preconditions
    let p = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if p > 0 { p as f64 } else { 4096.0 }
}

fn resident() -> Option<f64> {
    let s = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: f64 = s.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * page_size())
}

/// (size, available) bytes of the file system at `path`.
fn statvfs(path: &str) -> Option<(f64, f64)> {
    let c = std::ffi::CString::new(path).ok()?;
    // SAFETY: a zeroed statvfs is a valid out-buffer; `c` is a valid C
    // string for the call
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let f = st.f_frsize as f64;
    Some((st.f_blocks as f64 * f, st.f_bavail as f64 * f))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_values_are_escaped() {
        assert_eq!(labels(&[]), "");
        assert_eq!(
            labels(&[("a", "x"), ("b", "q\"\\\n")]),
            "{a=\"x\",b=\"q\\\"\\\\\\n\"}"
        );
    }

    #[test]
    fn memory_from_meminfo() {
        let s = "MemTotal:        507000 kB\nMemFree:  1 kB\nMemAvailable:    300000 kB\n";
        assert_eq!(
            meminfo(s),
            vec![
                ("total", 507000.0 * 1024.0),
                ("available", 300000.0 * 1024.0)
            ]
        );
    }

    #[test]
    fn the_root_file_system_has_a_size() {
        let (size, avail) = statvfs("/").unwrap();
        assert!(size > 0.0 && avail <= size);
        assert!(statvfs("/no/such/path").is_none());
    }
}
