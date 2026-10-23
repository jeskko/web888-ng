// SPDX-License-Identifier: GPL-3.0-or-later
//! Ordered-statistics decoding of the (174,91) code, for candidates that
//! belief propagation gives up on.
//!
//! The 91 most reliable independent bit positions (by |LLR|) are an
//! information set: taking their hard decisions, and re-encoding, gives
//! one codeword (order 0). Flipping one or two of the least reliable of
//! those positions gives more (orders 1 and 2). Among the candidates
//! whose CRC is right, the closest to the received soft bits wins: the
//! smallest sum of |LLR| over the bits it contradicts.

use crate::crc;
use crate::ldpc::{self, Bits, K, M, N};

/// How far to search.
#[derive(Clone, Copy, Debug)]
pub struct Depth {
    /// Flip each of the last `order1` information bits (up to 91).
    pub order1: usize,
    /// Flip each pair among the last `order2` information bits.
    pub order2: usize,
}

pub struct Found {
    pub codeword: Bits,
    /// Sum of |LLR| over the bits the codeword contradicts.
    pub distance: f32,
    /// Hard-decision bits the codeword contradicts.
    pub hard_errors: usize,
}

/// Generator rows (as codewords) in systematic order, built once.
fn generator() -> &'static [Bits; K] {
    static G: std::sync::OnceLock<[Bits; K]> = std::sync::OnceLock::new();
    G.get_or_init(|| {
        std::array::from_fn(|j| {
            let mut r = [0u64; 3];
            ldpc::set(&mut r, j, true);
            for k in 0..M {
                if ldpc::gen_bit(k, j) {
                    ldpc::set(&mut r, K + k, true);
                }
            }
            r
        })
    })
}

fn xor(a: &mut Bits, b: &Bits) {
    a[0] ^= b[0];
    a[1] ^= b[1];
    a[2] ^= b[2];
}

pub fn decode(llr: &[f32; N], depth: Depth) -> Option<Found> {
    let mut order: [usize; N] = std::array::from_fn(|i| i);
    order.sort_by(|&a, &b| llr[b].abs().total_cmp(&llr[a].abs()));

    // Row-reduce the generator so that the most reliable independent
    // columns become unit columns. Row operations keep the code, so the
    // rows stay codewords in the original bit order.
    let mut g = *generator();
    let mut pivots = [0usize; K];
    let mut r = 0;
    for &col in &order {
        if r == K {
            break;
        }
        let Some(p) = (r..K).find(|&i| ldpc::get(&g[i], col)) else {
            continue;
        };
        g.swap(r, p);
        let pr = g[r];
        for (i, row) in g.iter_mut().enumerate() {
            if i != r && ldpc::get(row, col) {
                xor(row, &pr);
            }
        }
        pivots[r] = col;
        r += 1;
    }
    if r < K {
        return None;
    }

    let mut hard = [0u64; 3];
    for (n, &l) in llr.iter().enumerate() {
        ldpc::set(&mut hard, n, l > 0.0);
    }
    let mut c0 = [0u64; 3];
    for (i, &col) in pivots.iter().enumerate() {
        if ldpc::get(&hard, col) {
            xor(&mut c0, &g[i]);
        }
    }

    let mut best: Option<(Bits, f32)> = None;
    let mut consider = |c: &Bits| {
        if crc::check(ldpc::message(c)).is_none_or(|m| m == 0) {
            return;
        }
        let d = distance(c, &hard, llr);
        if best.is_none_or(|(_, bd)| d < bd) {
            best = Some((*c, d));
        }
    };
    consider(&c0);
    let o1 = depth.order1.min(K);
    for ga in &g[K - o1..] {
        let mut c = c0;
        xor(&mut c, ga);
        consider(&c);
    }
    let o2 = depth.order2.min(K);
    for (i, ga) in g[K - o2..].iter().enumerate() {
        let mut ca = c0;
        xor(&mut ca, ga);
        for gb in &g[K - o2 + i + 1..] {
            let mut c = ca;
            xor(&mut c, gb);
            consider(&c);
        }
    }
    best.map(|(codeword, distance)| {
        let diff = [
            codeword[0] ^ hard[0],
            codeword[1] ^ hard[1],
            codeword[2] ^ hard[2],
        ];
        let hard_errors = diff.iter().map(|w| w.count_ones() as usize).sum();
        Found {
            codeword,
            distance,
            hard_errors,
        }
    })
}

fn distance(c: &Bits, hard: &Bits, llr: &[f32; N]) -> f32 {
    let mut d = 0.0;
    for w in 0..3 {
        let mut x = c[w] ^ hard[w];
        while x != 0 {
            let b = x.trailing_zeros() as usize;
            d += llr[w * 64 + b].abs();
            x &= x - 1;
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_codeword_with_unreliable_errors() {
        let m = 0x0bad_cafe_f00d_1234_5678u128 & ((1 << 77) - 1);
        let c = ldpc::encode(crc::with_crc(m));
        // Strong correct bits, plus 30 wrong ones that are weaker than
        // all the correct ones: order 0 already sees through them.
        let mut llr: [f32; N] = std::array::from_fn(|i| if ldpc::get(&c, i) { 3.0 } else { -3.0 });
        for i in (0..N).step_by(6) {
            llr[i] = -llr[i] * 0.1;
        }
        let f = decode(
            &llr,
            Depth {
                order1: 91,
                order2: 20,
            },
        )
        .unwrap();
        assert_eq!(f.codeword, c);
        assert_eq!(f.hard_errors, 29);
    }
}
