// SPDX-License-Identifier: GPL-3.0-or-later
//! The device's CPU load and temperature for the status bar: one thread
//! samples them every two seconds (started on first use), sessions read
//! the latest.

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const PERIOD: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SysStat {
    /// Busy share of all CPUs over the last period, percent.
    pub cpu: Option<f32>,
    /// SoC temperature, °C.
    pub temp_c: Option<f32>,
}

static LATEST: OnceLock<Mutex<SysStat>> = OnceLock::new();

/// The latest sample (none yet: all `None`).
pub fn current() -> SysStat {
    let m = LATEST.get_or_init(|| {
        std::thread::Builder::new()
            .name("sysstat".into())
            .spawn(sample_loop)
            .expect("spawn sysstat");
        Mutex::new(SysStat::default())
    });
    *m.lock().unwrap()
}

fn sample_loop() {
    let mut prev = cpu_times();
    loop {
        std::thread::sleep(PERIOD);
        let now = cpu_times();
        let s = SysStat {
            cpu: match (prev, now) {
                (Some(a), Some(b)) => busy_pct(a, b),
                _ => None,
            },
            temp_c: temperature(),
        };
        prev = now;
        if let Some(m) = LATEST.get() {
            *m.lock().unwrap() = s;
        }
    }
}

/// (busy, total) jiffies of all CPUs, from /proc/stat.
fn cpu_times() -> Option<(u64, u64)> {
    parse_stat(&std::fs::read_to_string("/proc/stat").ok()?)
}

fn parse_stat(s: &str) -> Option<(u64, u64)> {
    let line = s.lines().find(|l| l.starts_with("cpu "))?;
    let v: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|x| x.parse().ok())
        .collect();
    // user nice system idle iowait irq softirq steal (guest is in user)
    let total: u64 = v.iter().take(8).sum();
    let idle = v.get(3)? + v.get(4).copied().unwrap_or(0);
    Some((total - idle, total))
}

fn busy_pct(a: (u64, u64), b: (u64, u64)) -> Option<f32> {
    let dt = b.1.checked_sub(a.1).filter(|&d| d > 0)?;
    let db = b.0.saturating_sub(a.0);
    Some((100.0 * db as f64 / dt as f64) as f32)
}

/// The Zynq's XADC (IIO `in_temp0_*`), else the first thermal zone.
fn temperature() -> Option<f32> {
    let read = |p: &str| std::fs::read_to_string(p).ok()?.trim().parse::<f64>().ok();
    if let Ok(dirs) = std::fs::read_dir("/sys/bus/iio/devices") {
        for d in dirs.flatten() {
            let p = d.path();
            let p = p.to_string_lossy();
            if let (Some(raw), Some(scale)) = (
                read(&format!("{p}/in_temp0_raw")),
                read(&format!("{p}/in_temp0_scale")),
            ) {
                let off = read(&format!("{p}/in_temp0_offset")).unwrap_or(0.0);
                return Some(((raw + off) * scale / 1000.0) as f32);
            }
        }
    }
    read("/sys/class/thermal/thermal_zone0/temp").map(|m| (m / 1000.0) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_share_from_proc_stat() {
        let a = parse_stat("cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 1 2 3\n").unwrap();
        let b = parse_stat("cpu  160 0 70 880 70 0 0 0 0 0\n").unwrap();
        assert_eq!(a, (150, 1000));
        // 80 busy of 180
        assert!((busy_pct(a, b).unwrap() - 44.44).abs() < 0.01);
        assert_eq!(busy_pct(b, b), None);
    }
}
