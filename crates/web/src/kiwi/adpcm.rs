// SPDX-License-Identifier: GPL-3.0-or-later
//! IMA (DVI) ADPCM, 4 bits a sample, the way the Kiwi protocol packs it:
//! two codes a byte, the first sample in the low nibble.
//!
//! Written from the IMA recommended practice (step table, index
//! adjustment and the encoder that tracks the decoder's prediction); the
//! upstream server's encoder carries a licence with an advertising clause
//! and is not used. Audio keeps the state from packet to packet; a
//! waterfall line starts from zero and is clamped to 0..255 (bytes).

/// The IMA step sizes.
const STEPS: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];

/// Step index change per code magnitude (the sign bit doesn't matter).
const INDEX_STEP: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

/// Encoder (or decoder) state: the prediction and the step index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Adpcm {
    pub prev: i32,
    pub index: i32,
}

impl Adpcm {
    /// Apply `code` to the state as a decoder would; returns the sample.
    fn step(&mut self, code: u8, lo: i32, hi: i32) -> i32 {
        let step = STEPS[self.index as usize];
        // (|code| + 1/2) * step / 4, in the standard's shifts
        let mut d = step >> 3;
        if code & 4 != 0 {
            d += step;
        }
        if code & 2 != 0 {
            d += step >> 1;
        }
        if code & 1 != 0 {
            d += step >> 2;
        }
        if code & 8 != 0 {
            d = -d;
        }
        self.prev = (self.prev + d).clamp(lo, hi);
        self.index = (self.index + INDEX_STEP[(code & 7) as usize]).clamp(0, 88);
        self.prev
    }

    /// The code for sample `x`, the state moved on.
    fn code(&mut self, x: i32, lo: i32, hi: i32) -> u8 {
        let mut diff = x - self.prev;
        let mut step = STEPS[self.index as usize];
        let mut code = 0u8;
        if diff < 0 {
            code = 8;
            diff = -diff;
        }
        for bit in [4u8, 2, 1] {
            if diff >= step {
                code |= bit;
                diff -= step;
            }
            step >>= 1;
        }
        self.step(code, lo, hi);
        code
    }

    /// Encode 16-bit samples (an even number) into `out`, two a byte.
    pub fn encode_i16(&mut self, x: &[i16], out: &mut Vec<u8>) {
        let (lo, hi) = (i16::MIN as i32, i16::MAX as i32);
        for p in x.chunks(2) {
            let a = self.code(p[0] as i32, lo, hi);
            let b = p.get(1).map_or(0, |&v| self.code(v as i32, lo, hi));
            out.push(a | (b << 4));
        }
    }

    /// Encode bytes (waterfall levels) into `out`, two a byte.
    pub fn encode_u8(&mut self, x: &[u8], out: &mut Vec<u8>) {
        for p in x.chunks(2) {
            let a = self.code(p[0] as i32, 0, 255);
            let b = p.get(1).map_or(0, |&v| self.code(v as i32, 0, 255));
            out.push(a | (b << 4));
        }
    }

    /// Decode (tests, and clients written against this module).
    pub fn decode(&mut self, x: &[u8], lo: i32, hi: i32) -> Vec<i32> {
        x.iter()
            .flat_map(|&b| [b & 15, b >> 4])
            .map(|c| self.step(c, lo, hi))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_round_trips_within_a_few_percent() {
        let x: Vec<i16> = (0..4096)
            .map(|k| (8000.0 * (k as f32 * 0.05).sin() + 3000.0 * (k as f32 * 0.31).sin()) as i16)
            .collect();
        let mut enc = Adpcm::default();
        let mut b = Vec::new();
        // in two packets: the state carries over
        enc.encode_i16(&x[..2048], &mut b);
        enc.encode_i16(&x[2048..], &mut b);
        assert_eq!(b.len(), 2048);
        let y = Adpcm::default().decode(&b, -32768, 32767);
        let err: f64 = x[200..]
            .iter()
            .zip(&y[200..])
            .map(|(&a, &b)| ((a as i32 - b) as f64).powi(2))
            .sum::<f64>();
        let sig: f64 = x[200..].iter().map(|&a| (a as f64).powi(2)).sum();
        // better than 20 dB SNR once the step has adapted
        assert!(err / sig < 0.01, "{}", err / sig);
    }

    #[test]
    fn first_sample_goes_in_the_low_nibble() {
        let mut e = Adpcm::default();
        let mut b = Vec::new();
        e.encode_i16(&[100, -100], &mut b);
        // +100 from 0 with step 7: all magnitude bits; then negative
        assert_eq!(b[0] & 15, 7);
        assert_eq!(b[0] >> 4 & 8, 8);
    }

    #[test]
    fn waterfall_levels_stay_bytes() {
        let x: Vec<u8> = (0..1034)
            .map(|k| if k % 50 < 3 { 250 } else { 60 })
            .collect();
        let mut b = Vec::new();
        Adpcm::default().encode_u8(&x, &mut b);
        assert_eq!(b.len(), 517);
        let y = Adpcm::default().decode(&b, 0, 255);
        assert!(y.iter().all(|&v| (0..=255).contains(&v)));
        // the floor is followed closely
        let off = y[500..540]
            .iter()
            .zip(&x[500..540])
            .filter(|&(_, &x)| x == 60)
            .map(|(&y, _)| (y - 60).abs())
            .max()
            .unwrap();
        assert!(off < 25, "{off}");
    }
}
