// SPDX-License-Identifier: GPL-3.0-or-later
//! Channel math shared by backends, the simulator and the DSP chain.
//!
//! RX chain per channel (FPGA): NCO mix `x * exp(-j w n)` (a signal at
//! `f_nco + d` appears at `+d`), CIC1 N=3 R=256, CIC2 N=5 R=r2, gain
//! normalised so that a full-scale DC input gives `RX_DC_FULL_SCALE`
//! independent of r2. WF chain: NCO mix, CIC N=5 R=decim, s16.

use std::f64::consts::PI;

/// Stock HF ADC clock.
pub const ADC_HZ_HF: f64 = 122_880_000.0;
/// Fixed first-stage RX decimation.
pub const RX_R1: u32 = 256;
pub const RX_CIC1_N: i32 = 3;
pub const RX_CIC2_N: i32 = 5;
pub const WF_CIC_N: i32 = 5;

/// RX output for a full-scale DC input (s32 samples, 1 bit headroom).
pub const RX_DC_FULL_SCALE: f64 = (1u64 << 30) as f64;
/// RX |I + jQ| for a full-scale real sine at the ADC (0 dBFS).
pub const RX_SINE_FULL_SCALE: f64 = (1u64 << 29) as f64;
/// WF |I + jQ| for a full-scale real sine at the ADC (0 dBFS).
pub const WF_SINE_FULL_SCALE: f64 = (1u64 << 14) as f64;
/// Waterfall one-shot block length (samples).
pub const WF_BLOCK: usize = 8192;

const TWO48: f64 = (1u64 << 48) as f64;
const MASK48: u64 = (1u64 << 48) - 1;

/// 48-bit NCO phase increment for `f_hz` at sample rate `fs`
/// (negative or above fs wrap modulo fs).
pub fn nco_word(f_hz: f64, fs: f64) -> u64 {
    let x = (f_hz / fs).rem_euclid(1.0) * TWO48;
    (x.round() as u64) & MASK48
}

/// Frequency actually produced by NCO word `w` (in [0, fs)).
pub fn nco_freq(w: u64, fs: f64) -> f64 {
    (w & MASK48) as f64 / TWO48 * fs
}

/// RX output sample rate for stage-2 decimation `r2`.
pub fn rx_rate(fs: f64, r2: u16) -> f64 {
    fs / (RX_R1 as f64 * r2 as f64)
}

/// Stage-2 decimation the stock server would use for an audio rate.
pub fn r2_for_rate(fs: f64, rate: f64) -> u16 {
    ((fs / rate) as u32 / RX_R1) as u16
}

/// Decode the signature word: (RX channels, WF channels).
pub fn signature_counts(sig: u32) -> (usize, usize) {
    ((sig & 0xf) as usize, ((sig >> 8) & 0xf) as usize)
}

/// |H| of an N-stage CIC with decimation R at input rate `fs_in`,
/// normalised to 1 at DC, evaluated at `f` Hz.
pub fn cic_mag(f: f64, fs_in: f64, r: u32, n: i32) -> f64 {
    let x = PI * f / fs_in;
    let den = (r as f64) * x.sin();
    if den.abs() < 1e-300 {
        return 1.0;
    }
    ((x * r as f64).sin() / den).abs().powi(n)
}

/// Combined RX channel magnitude response at offset `d` Hz from the NCO.
pub fn rx_chain_mag(d: f64, fs: f64, r2: u16) -> f64 {
    cic_mag(d, fs, RX_R1, RX_CIC1_N) * cic_mag(d, fs / RX_R1 as f64, r2 as u32, RX_CIC2_N)
}

/// WF channel magnitude response at offset `d` Hz from the NCO.
pub fn wf_chain_mag(d: f64, fs: f64, decim: u16) -> f64 {
    cic_mag(d, fs, decim as u32, WF_CIC_N)
}

/// Fold an offset into the first Nyquist zone [-fs_out/2, fs_out/2).
pub fn alias(d: f64, fs_out: f64) -> f64 {
    (d + fs_out / 2.0).rem_euclid(fs_out) - fs_out / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nco_word_roundtrip() {
        let fs = ADC_HZ_HF;
        for f in [0.0, 7.0745e6, 14.2005e6, 61.0e6, -1000.0] {
            let w = nco_word(f, fs);
            let back = nco_freq(w, fs);
            let want = f.rem_euclid(fs);
            assert!((back - want).abs() < fs / TWO48, "{f}: {back} vs {want}");
        }
        assert_eq!(nco_word(fs / 2.0, fs), 1u64 << 47);
    }

    #[test]
    fn rates_match_stock_table() {
        // design/server-fpga-interface table: 12k -> 40, 24k -> 20, 48k -> 10
        assert_eq!(r2_for_rate(ADC_HZ_HF, 12_000.0), 40);
        assert_eq!(r2_for_rate(ADC_HZ_HF, 24_000.0), 20);
        assert_eq!(r2_for_rate(ADC_HZ_HF, 48_000.0), 10);
        assert_eq!(rx_rate(ADC_HZ_HF, 40), 12_000.0);
    }

    #[test]
    fn signature() {
        assert_eq!(signature_counts(0xAA55_020C), (12, 2)); // stock HF
        assert_eq!(signature_counts(0x0E5D_020D), (13, 2)); // open VHF
    }

    #[test]
    fn cic_response() {
        assert!((cic_mag(0.0, 1.0, 8, 5) - 1.0).abs() < 1e-12);
        // nulls at multiples of fs_in / R
        assert!(cic_mag(1.0 / 8.0, 1.0, 8, 5) < 1e-12);
        // RX chain at the output Nyquist edge, r2 = 40: N=5 CIC droop
        // ~ (2/pi)^5 -> -19.6 dB (CIC1 adds < 0.01 dB)
        let fs_out = rx_rate(ADC_HZ_HF, 40);
        let db = 20.0 * rx_chain_mag(fs_out / 2.0, ADC_HZ_HF, 40).log10();
        assert!((db + 19.6).abs() < 0.1, "{db}");
    }

    #[test]
    fn alias_fold() {
        assert_eq!(alias(1000.0, 12000.0), 1000.0);
        assert_eq!(alias(7000.0, 12000.0), -5000.0);
        assert_eq!(alias(-7000.0, 12000.0), 5000.0);
    }
}
