// SPDX-License-Identifier: GPL-3.0-or-later
//! The (174,91) LDPC code of FT8/FT4: systematic encoder and a
//! belief propagation decoder (normalised min-sum).
//!
//! Codeword bit 0 is the first bit sent. Bits 0..91 are the message and
//! CRC (bit 90 of the `u128` word is codeword bit 0), bits 91..174 the
//! parity. Log-likelihood ratios are ln(P(1) / P(0)): positive means 1.

use crate::tables::{GENERATOR, MN, NM, NUM_ROWS};

pub const N: usize = 174;
pub const K: usize = 91;
pub const M: usize = N - K;

/// Codeword bits as a bit set (bit i of word i / 64).
pub type Bits = [u64; 3];

pub fn get(b: &Bits, i: usize) -> bool {
    b[i / 64] >> (i % 64) & 1 != 0
}

pub fn set(b: &mut Bits, i: usize, v: bool) {
    if v {
        b[i / 64] |= 1 << (i % 64);
    } else {
        b[i / 64] &= !(1 << (i % 64));
    }
}

/// Generator row `k` (parity bit 91 + k) as a 91-bit word.
fn gen_row(k: usize) -> u128 {
    let mut r = 0u128;
    for j in 0..K {
        if GENERATOR[k][j / 8] >> (7 - j % 8) & 1 != 0 {
            r |= 1 << (K - 1 - j);
        }
    }
    r
}

fn gen_rows() -> &'static [u128; M] {
    static ROWS: std::sync::OnceLock<[u128; M]> = std::sync::OnceLock::new();
    ROWS.get_or_init(|| std::array::from_fn(gen_row))
}

/// Parity bit `k` depends on message bit `j` (both 0-based).
pub fn gen_bit(k: usize, j: usize) -> bool {
    gen_rows()[k] >> (K - 1 - j) & 1 != 0
}

/// Encode 91 bits (message + CRC) into a codeword.
pub fn encode(a91: u128) -> Bits {
    let mut c = [0u64; 3];
    for j in 0..K {
        set(&mut c, j, a91 >> (K - 1 - j) & 1 != 0);
    }
    for (k, row) in gen_rows().iter().enumerate() {
        set(&mut c, K + k, (row & a91).count_ones() & 1 != 0);
    }
    c
}

/// The 91 message bits of a codeword.
pub fn message(c: &Bits) -> u128 {
    let mut a = 0u128;
    for j in 0..K {
        a = a << 1 | get(c, j) as u128;
    }
    a
}

/// Number of failed parity checks.
pub fn check(c: &Bits) -> usize {
    (0..M)
        .filter(|&m| {
            let mut x = false;
            for &n in &NM[m][..NUM_ROWS[m] as usize] {
                x ^= get(c, n as usize - 1);
            }
            x
        })
        .count()
}

/// Scale of the min-sum check messages. Min-sum overstates them; 0.8
/// decodes the corpus as sum-product did (doc/m4-skimmers.md §3.3) at
/// half the A9's time (no tanh, atanh or divisions).
const ALPHA: f32 = 0.8;

/// Belief propagation decoder state; reusable between candidates.
pub struct Bp {
    /// Each check's bits (0-based) and, for each, which of the bit's
    /// three checks it is.
    nm: [[(u8, u8); 7]; M],
    tov: [[f32; 3]; N],
}

impl Default for Bp {
    fn default() -> Self {
        let mut nm = [[(0u8, 0u8); 7]; M];
        for m in 0..M {
            for i in 0..NUM_ROWS[m] as usize {
                let n = NM[m][i] as usize - 1;
                let k = MN[n].iter().position(|&x| x as usize - 1 == m).unwrap();
                nm[m][i] = (n as u8, k as u8);
            }
        }
        Bp {
            nm,
            tov: [[0.0; 3]; N],
        }
    }
}

/// Result of a decoding attempt.
pub struct Decoded {
    pub codeword: Bits,
    /// Parity checks failed by the best hard decision (0: a codeword).
    pub errors: usize,
    pub iterations: usize,
}

impl Bp {
    /// Decode `llr` with up to `max_iter` iterations. The result is a
    /// codeword when `errors == 0`. As in WSJT-X, it gives up when the
    /// failed checks haven't decreased for 5 iterations, after 10 and
    /// with more than 15 failing: such a candidate rarely converges.
    pub fn decode(&mut self, llr: &[f32; N], max_iter: usize) -> Decoded {
        for t in self.tov.iter_mut() {
            *t = [0.0; 3];
        }
        let mut best = Decoded {
            codeword: [0; 3],
            errors: M,
            iterations: 0,
        };
        let mut zn = *llr;
        let (mut last, mut stalled) = (M, 0);
        for iter in 0..=max_iter {
            let mut c = [0u64; 3];
            for (n, &z) in zn.iter().enumerate() {
                set(&mut c, n, z > 0.0);
            }
            let errors = check(&c);
            stalled = if errors < last { 0 } else { stalled + 1 };
            last = errors;
            if errors < best.errors {
                best = Decoded {
                    codeword: c,
                    errors,
                    iterations: iter,
                };
                if errors == 0 {
                    break;
                }
            }
            if iter == max_iter || (stalled >= 5 && iter >= 10 && errors > 15) {
                break;
            }
            // Each check tells each of its bits what the others say: the
            // smallest of their magnitudes, and the sign that makes the
            // check hold. LLRs are ln(P1 / P0), so an even number of ones
            // is the product of the others' signs times (-1)^(w - 1).
            for (m, &w) in NUM_ROWS.iter().enumerate() {
                let w = w as usize;
                // Bits to the check: everything a bit knows except what
                // this check told it.
                let mut v = [0f32; 7];
                let mut sign = if w % 2 == 1 { -1.0f32 } else { 1.0 };
                let (mut min1, mut min2, mut imin) = (f32::MAX, f32::MAX, 0);
                for (i, vi) in v[..w].iter_mut().enumerate() {
                    let (n, k) = self.nm[m][i];
                    let t = zn[n as usize] - self.tov[n as usize][k as usize];
                    *vi = t;
                    if t < 0.0 {
                        sign = -sign;
                    }
                    let a = t.abs();
                    if a < min1 {
                        (min2, min1, imin) = (min1, a, i);
                    } else if a < min2 {
                        min2 = a;
                    }
                }
                for (i, &t) in v[..w].iter().enumerate() {
                    let (n, k) = self.nm[m][i];
                    let mag = if i == imin { min2 } else { min1 };
                    let s = if t < 0.0 { -sign } else { sign };
                    self.tov[n as usize][k as usize] = ALPHA * s * mag;
                }
            }
            for n in 0..N {
                zn[n] = llr[n] + self.tov[n].iter().sum::<f32>();
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crc;

    fn llr_of(c: &Bits, a: f32) -> [f32; N] {
        std::array::from_fn(|i| if get(c, i) { a } else { -a })
    }

    #[test]
    fn codewords_pass_every_check() {
        for seed in 0..50u128 {
            let m =
                (seed.wrapping_mul(0x9E37_79B9_7F4A_7C15_F39C_C060_5CED_C834)) & ((1 << 77) - 1);
            let c = encode(crc::with_crc(m));
            assert_eq!(check(&c), 0);
            assert_eq!(crc::check(message(&c)), Some(m));
        }
    }

    #[test]
    fn matches_ft8code_parity() {
        // ft8code "CQ K1ABC FN42": 77 message bits, CRC, 83 parity bits
        let m = u128::from_str_radix(
            "00000000000000000000000000100000010011011110111100011010100010100001100110001",
            2,
        )
        .unwrap();
        let c = encode(crc::with_crc(m));
        let parity: String = (K..N).map(|i| if get(&c, i) { '1' } else { '0' }).collect();
        assert_eq!(
            parity,
            "10101000001001000110111100001111000000111010010110111110100110100100001010010100110"
        );
    }

    #[test]
    fn bp_corrects_errors() {
        let m = 0x1234_5678_9abc_def0_1234u128 & ((1 << 77) - 1);
        let c = encode(crc::with_crc(m));
        let mut llr = llr_of(&c, 4.0);
        // 20 weak wrong bits
        for i in (0..N).step_by(9) {
            llr[i] = -llr[i] * 0.3;
        }
        let d = Bp::default().decode(&llr, 30);
        assert_eq!(d.errors, 0);
        assert_eq!(d.codeword, c);
    }
}
