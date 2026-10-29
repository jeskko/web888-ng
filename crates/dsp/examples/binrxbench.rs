// SPDX-License-Identifier: GPL-3.0-or-later
//! `BinRx` speed: N tuned channels (each in its own bin) from a synthetic
//! bin source at 60k frames/s, RX at 48 kS/s; prints the speed against
//! real time and the share of a core per channel (from the process's CPU
//! time where /proc has it: a bench beside a running server is preempted).
//!   cargo run --release -p web888-dsp --example binrxbench [channels...]
use std::sync::Arc;
use std::time::Instant;
use web888_dsp::binrx::BinRx;
use web888_radio::bins::{BinCaps, BinFrame, BinRadio, selection};
use web888_radio::*;

struct Src {
    sel: Arc<[usize]>,
    frame: u64,
}

impl BinRadio for Src {
    fn bin_caps(&self) -> BinCaps {
        BinCaps {
            bin_hz: 30e3,
            frame_rate: 60e3,
            bins: 2047,
            first_hz: 0.0,
            wraps: false,
            max_selected: 64,
        }
    }
    fn set_bins(&mut self, bins: &[usize]) -> Result<u8> {
        self.sel = Arc::from(selection(bins, 2047, 64));
        Ok(1)
    }
    fn read_bins(&mut self, out: &mut Vec<BinFrame>) -> Result<usize> {
        for _ in 0..600 {
            let x = (self.frame as i32).wrapping_mul(2654435761u32 as i32) >> 4;
            out.push(BinFrame {
                frame: self.frame,
                generation: 1,
                bins: self.sel.clone(),
                iq: vec![Iq32 { i: x, q: -x }; self.sel.len()],
            });
            self.frame += 1;
        }
        Ok(600)
    }
    fn set_power(&mut self, _f: u16) -> Result<()> {
        Ok(())
    }
    fn read_power(&mut self, _l: &mut Vec<f32>) -> Result<Option<u32>> {
        Ok(None)
    }
}

impl Radio for Src {
    fn caps(&self) -> Caps {
        Caps {
            variant: Variant::Hf,
            rx_channels: 0,
            wf_channels: 0,
            adc_hz: ADC_HZ_HF,
            rx_r2: 5..=40,
            wf_decim_min: 1,
            wf_decim_max: 8192,
            wf_block: 8192,
            sample_counter: false,
            rf_range: 0.0..=61.44e6,
        }
    }
    fn adc_hz(&self) -> f64 {
        ADC_HZ_HF
    }
    fn set_adc_hz(&mut self, _hz: f64) {}
    fn set_rx_rate(&mut self, _r2: u16) -> Result<()> {
        Ok(())
    }
    fn rx_r2(&self) -> u16 {
        0
    }
    fn tune_rx(&mut self, ch: usize, _f: f64) -> Result<()> {
        Err(Error::InvalidChannel(ch))
    }
    fn read_rx(&mut self, _b: &mut [Iq32]) -> Result<usize> {
        Ok(0)
    }
    fn start_wf(&mut self, s: usize, _f: f64, _d: u16, _m: WfMode) -> Result<()> {
        Err(Error::InvalidChannel(s))
    }
    fn read_wf(&mut self, s: usize, _b: &mut [Iq16]) -> Result<usize> {
        Err(Error::InvalidChannel(s))
    }
    fn front_end(&mut self, _fe: FrontEnd) -> Result<()> {
        Ok(())
    }
    fn pps(&mut self) -> Result<Option<u32>> {
        Ok(None)
    }
    fn sample_counter(&mut self) -> Option<u64> {
        None
    }
    fn identity(&self) -> Identity {
        Identity {
            backend: "bench",
            signature: 0,
            dna: None,
            build_id: None,
            live: false,
        }
    }
}

/// The process's user + system CPU time (s), from /proc/self/stat.
fn cpu_secs() -> Option<f64> {
    let s = std::fs::read_to_string("/proc/self/stat").ok()?;
    let rest = &s[s.rfind(')')? + 2..];
    let f: Vec<&str> = rest.split(' ').collect();
    // fields 14 and 15 of the line: here 11 and 12 after the state
    let t: f64 = f.get(11)?.parse::<f64>().ok()? + f.get(12)?.parse::<f64>().ok()?;
    Some(t / 100.0) // USER_HZ
}

fn main() {
    let counts: Vec<usize> = std::env::args()
        .skip(1)
        .filter_map(|s| s.parse().ok())
        .collect();
    let counts = if counts.is_empty() {
        vec![0, 1, 4, 16, 32]
    } else {
        counts
    };
    for n in counts {
        let src = Src {
            sel: Arc::from(Vec::new()),
            frame: 0,
        };
        let mut r = BinRx::new(src, 32).unwrap();
        for c in 0..n {
            r.tune_rx(c, 7.0e6 + 30e3 * c as f64 + 1234.0).unwrap();
        }
        let mut buf = vec![Iq32::default(); 32 * 512];
        let secs = 2.0;
        let want = (48e3 * secs) as usize;
        let t = Instant::now();
        let c0 = cpu_secs();
        let mut got = 0;
        while got < want {
            got += r.read_rx(&mut buf).unwrap();
        }
        let dt = t.elapsed().as_secs_f64();
        let used = match (c0, cpu_secs()) {
            (Some(a), Some(b)) => b - a,
            _ => dt,
        };
        let core = used / secs;
        println!(
            "{n:2} tuned of 32: {:.1}x real time (wall), {:.1} % of a core (CPU time){}",
            secs / dt,
            100.0 * core,
            if n > 0 {
                format!(", {:.2} % per channel", 100.0 * core / n as f64)
            } else {
                String::new()
            }
        );
    }
}
