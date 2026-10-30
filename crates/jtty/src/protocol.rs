// SPDX-License-Identifier: GPL-3.0-or-later
//! The frame: 34 payload bits, CRC-12, a rate-1/2 tail-biting code (K =
//! 10, generators 1167 and 1545 octal) and 4-GFSK tones (13 sync + 46
//! data symbols, 31.25 Hz apart, 32 baud), and its waveform (jtty-cuf
//! `protocol.cpp`, `message_codec.cpp`).

pub const SAMPLE_RATE_HZ: u32 = 12_000;
pub const SYMBOLS_PER_FRAME: usize = 59;
pub const SAMPLES_PER_SYMBOL: usize = 384;
pub const SAMPLES_PER_FRAME: usize = SYMBOLS_PER_FRAME * SAMPLES_PER_SYMBOL;
pub const TONE_SPACING_HZ: f64 = 31.25;
pub const MIN_BASE_FREQUENCY_HZ: i32 = 200;
pub const MAX_BASE_FREQUENCY_HZ: i32 = 5000;

/// Payload (34), information (payload + CRC-12) and coded bits, one per
/// byte (0 or 1).
pub type PayloadBits = [u8; 34];
pub type InfoBits = [u8; 46];
pub type CodedBits = [u8; 92];
pub type ToneFrame = [u8; 59];

pub(crate) const ALPHABET: &[u8; 64] =
    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ +-./?!\"#$%,&*()_'=[]{}<>|:;";
pub const SYNC_TONES: [u8; 13] = [0, 2, 2, 3, 0, 0, 3, 2, 1, 3, 1, 2, 0];
const MAX_WAVEFORM_FRAMES: usize = 16;

pub(crate) fn alphabet_index(ch: u8) -> Option<usize> {
    ALPHABET.iter().position(|&a| a == ch)
}

pub(crate) fn crc_step(reg: u16, bit: u8) -> u16 {
    let reg = reg ^ ((bit as u16) << 11);
    let reg = if reg & 0x800 != 0 {
        (reg << 1) ^ 0x80F
    } else {
        reg << 1
    };
    reg & 0xFFF
}

pub(crate) fn crc_valid(info: &InfoBits) -> bool {
    info.iter().fold(0, |r, &b| crc_step(r, b)) == 0
}

pub(crate) fn next_state(state: u16, bit: u8) -> u16 {
    ((state << 1) | bit as u16) & 0x1FF
}

pub(crate) fn code_output(state: u16, bit: u8) -> [u8; 2] {
    let reg = ((state as u32) << 1) | bit as u32;
    [
        ((reg & 0o1167).count_ones() & 1) as u8,
        ((reg & 0o1545).count_ones() & 1) as u8,
    ]
}

/// Coded bit pairs 00, 01, 11, 10 are tones 0, 1, 2, 3.
pub(crate) const PAIR_TO_TONE: [u8; 4] = [0, 1, 3, 2];

/// A frame's wire form.
#[derive(Clone, Debug, PartialEq)]
pub struct EncodedFrame {
    pub payload: PayloadBits,
    pub information: InfoBits,
    pub coded: CodedBits,
    pub tones: ToneFrame,
    pub message_word: u32,
    pub end_of_message: bool,
}

pub(crate) fn word_from_payload(payload: &PayloadBits) -> u32 {
    payload[..32].iter().fold(0u32, |w, &b| (w << 1) | b as u32)
}

pub(crate) fn payload_from_word(word: u32, eom: bool) -> PayloadBits {
    let mut p = [0u8; 34];
    for (i, b) in p.iter_mut().take(32).enumerate() {
        *b = ((word >> (31 - i)) & 1) as u8;
    }
    p[33] = eom as u8;
    p
}

/// The whole pipeline for a 34-bit payload: CRC, tail-biting code, tones.
pub fn encode_payload(payload: &PayloadBits) -> Result<EncodedFrame, String> {
    if payload.iter().any(|&b| b > 1) {
        return Err("payload bits must contain only 0 or 1".into());
    }
    let mut information = [0u8; 46];
    information[..34].copy_from_slice(payload);
    let remainder = payload.iter().fold(0, |r, &b| crc_step(r, b));
    for i in 0..12 {
        information[34 + i] = ((remainder >> (11 - i)) & 1) as u8;
    }
    // tail-biting: the register starts as the last nine information bits
    let mut state = 0u16;
    for &b in &information[37..46] {
        state = next_state(state, b);
    }
    let mut coded = [0u8; 92];
    for (i, &b) in information.iter().enumerate() {
        let out = code_output(state, b);
        coded[2 * i] = out[0];
        coded[2 * i + 1] = out[1];
        state = next_state(state, b);
    }
    let mut tones = [0u8; 59];
    tones[..13].copy_from_slice(&SYNC_TONES);
    for i in 0..46 {
        tones[13 + i] = PAIR_TO_TONE[((coded[2 * i] << 1) | coded[2 * i + 1]) as usize];
    }
    Ok(EncodedFrame {
        payload: *payload,
        information,
        coded,
        tones,
        message_word: word_from_payload(payload),
        end_of_message: payload[33] != 0,
    })
}

/// erf to about 1e-12 (as web888-ftx's: a series, then erfc's continued
/// fraction).
pub(crate) fn erf(x: f64) -> f64 {
    if x.abs() < 2.5 {
        let mut sum = x;
        let mut term = x;
        let x2 = x * x;
        for n in 1..60 {
            term *= -x2 / n as f64;
            sum += term / (2 * n + 1) as f64;
        }
        sum * 2.0 / std::f64::consts::PI.sqrt()
    } else {
        let s = x.signum();
        let x = x.abs();
        let mut f = x;
        let mut c = x;
        let mut d = 0.0;
        for k in 1..80 {
            let a = k as f64 / 2.0;
            d = 1.0 / (x + a * d);
            c = x + a / c;
            f *= c * d;
        }
        let erfc = (-x * x).exp() / (f * std::f64::consts::PI.sqrt());
        s * (1.0 - erfc)
    }
}

/// The frequency pulse, BT = 2, at `t` symbols from the symbol's centre.
pub(crate) fn gaussian_pulse(t: f64) -> f64 {
    let c = std::f64::consts::PI * (2.0 / std::f64::consts::LN_2).sqrt();
    0.5 * (erf(c * 2.0 * (t + 0.5)) - erf(c * 2.0 * (t - 0.5)))
}

/// IEEE remainder (the quotient rounded to even), as C's `remainder`.
pub(crate) fn remainder(x: f64, y: f64) -> f64 {
    x - (x / y).round_ties_even() * y
}

/// The waveform of one to sixteen frames at 12 kHz: continuous phase,
/// Gaussian shaping across frame boundaries, a raised-cosine envelope at
/// the two ends only. `base_frequency_hz` (tone 0) is a whole number from
/// 200 to 5000.
pub fn modulate_frames(
    frames: &[ToneFrame],
    base_frequency_hz: f64,
    amplitude: f64,
) -> Result<Vec<f32>, String> {
    if frames.is_empty() || frames.len() > MAX_WAVEFORM_FRAMES {
        return Err("frame count is outside the 1..16 waveform bound".into());
    }
    if !base_frequency_hz.is_finite()
        || base_frequency_hz < MIN_BASE_FREQUENCY_HZ as f64
        || base_frequency_hz > MAX_BASE_FREQUENCY_HZ as f64
        || base_frequency_hz.trunc() != base_frequency_hz
    {
        return Err("base frequency must be an integer from 200 through 5000 Hz".into());
    }
    if !amplitude.is_finite() || amplitude <= 0.0 || amplitude > 1.0 {
        return Err("amplitude must be finite and in (0, 1]".into());
    }
    if frames.iter().flatten().any(|&t| t > 3) {
        return Err("tone index exceeds 3".into());
    }
    let symbol_samples = SAMPLES_PER_SYMBOL;
    let total_symbols = frames.len() * SYMBOLS_PER_FRAME;
    let mut output = vec![0f32; total_symbols * symbol_samples];
    let tone_at = |index: i64| -> f64 {
        if index < 0 {
            frames[0][0] as f64
        } else if index as usize >= total_symbols {
            frames[frames.len() - 1][SYMBOLS_PER_FRAME - 1] as f64
        } else {
            let a = index as usize;
            frames[a / SYMBOLS_PER_FRAME][a % SYMBOLS_PER_FRAME] as f64
        }
    };
    let ramp = symbol_samples / 8;
    let rate = SAMPLE_RATE_HZ as f64;
    let two_pi = 2.0 * std::f64::consts::PI;
    let n_out = output.len();
    let mut phase = 0.0f64;
    for (n, out) in output.iter_mut().enumerate() {
        let symbol = (n / symbol_samples) as i64;
        let local = ((n % symbol_samples) as f64 + 0.5) / symbol_samples as f64 - 0.5;
        let shaped = tone_at(symbol - 1) * gaussian_pulse(local + 1.0)
            + tone_at(symbol) * gaussian_pulse(local)
            + tone_at(symbol + 1) * gaussian_pulse(local - 1.0);
        let mut envelope = 1.0;
        if n < ramp {
            envelope = 0.5 * (1.0 - (std::f64::consts::PI * n as f64 / ramp as f64).cos());
        } else if n >= n_out - ramp {
            let remaining = n_out - 1 - n;
            envelope = 0.5 * (1.0 - (std::f64::consts::PI * remaining as f64 / ramp as f64).cos());
        }
        *out = (amplitude * envelope * phase.sin()) as f32;
        phase = remainder(
            phase + two_pi * (base_frequency_hz + TONE_SPACING_HZ * shaped) / rate,
            two_pi,
        );
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erf_is_accurate() {
        assert!((erf(0.5) - 0.520_499_877_813_046_5).abs() < 1e-12);
        assert!((erf(3.0) - 0.999_977_909_503_001_4).abs() < 1e-12);
    }

    #[test]
    fn a_frame_starts_with_the_sync_word_and_checks() {
        let p = payload_from_word(0x1234_5677, true);
        let f = encode_payload(&p).unwrap();
        assert_eq!(&f.tones[..13], &SYNC_TONES);
        assert!(crc_valid(&f.information));
        assert!(f.end_of_message);
        assert_eq!(f.message_word, 0x1234_5677);
    }
}
