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
//!
//! The CRC is linear, so each row's CRC syndrome is computed once, and a
//! combination of rows passes the CRC exactly when their syndromes XOR to
//! that of the order-0 word. Pairs and triples are then found by looking
//! up the syndrome that would complete them, instead of trying them all.

use crate::crc;
use crate::ldpc::{self, Bits, K, M, N};

/// How far to search.
#[derive(Clone, Copy, Debug)]
pub struct Depth {
    /// Flip each of the last `order1` information bits (up to 91).
    pub order1: usize,
    /// Flip each pair among the last `order2` information bits.
    pub order2: usize,
    /// Flip each triple among the last `order3` information bits.
    pub order3: usize,
}

pub struct Found {
    pub codeword: Bits,
    /// Sum of |LLR| over the bits the codeword contradicts.
    pub distance: f32,
    /// Hard-decision bits the codeword contradicts.
    pub hard_errors: usize,
    /// Distance of the next closest CRC-valid codeword seen (infinite if
    /// none): a real decode stands out from the chance ones.
    pub second: f32,
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

    // CRC syndromes: zero for a word whose CRC is right.
    let syn: [u16; K] = std::array::from_fn(|i| syndrome(&g[i]));
    let s0 = syndrome(&c0);

    let mut best: Option<(Bits, f32)> = None;
    let mut second = f32::INFINITY;
    let mut consider = |rows: &[usize]| {
        let mut c = c0;
        for &i in rows {
            xor(&mut c, &g[i]);
        }
        if crc::check(ldpc::message(&c)).is_none_or(|m| m == 0) {
            return;
        }
        let d = distance(&c, &hard, llr);
        match best {
            Some((_, bd)) if d >= bd => second = second.min(d),
            Some((_, bd)) => {
                second = bd;
                best = Some((c, d));
            }
            None => best = Some((c, d)),
        }
    };
    if s0 == 0 {
        consider(&[]);
    }
    let o1 = depth.order1.min(K);
    for (a, &sa) in syn.iter().enumerate().skip(K - o1) {
        if s0 ^ sa == 0 {
            consider(&[a]);
        }
    }
    // Rows by syndrome, among the last `n` information bits.
    let lookup = |n: usize| {
        let mut head = vec![u8::MAX; 1 << 14];
        let mut next = [u8::MAX; K];
        for i in K - n..K {
            next[i] = head[syn[i] as usize];
            head[syn[i] as usize] = i as u8;
        }
        (head, next)
    };
    let o2 = depth.order2.min(K);
    if o2 >= 2 {
        let (head, next) = lookup(o2);
        for a in K - o2..K {
            let mut b = head[(s0 ^ syn[a]) as usize];
            while b != u8::MAX {
                if b as usize > a {
                    consider(&[a, b as usize]);
                }
                b = next[b as usize];
            }
        }
    }
    let o3 = depth.order3.min(K);
    if o3 >= 3 {
        let (head, next) = lookup(o3);
        for a in K - o3..K {
            for b in a + 1..K {
                let mut c = head[(s0 ^ syn[a] ^ syn[b]) as usize];
                while c != u8::MAX {
                    if c as usize > b {
                        consider(&[a, b, c as usize]);
                    }
                    c = next[c as usize];
                }
            }
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
            second,
        }
    })
}

/// CRC of the message bits against the CRC bits sent.
fn syndrome(c: &Bits) -> u16 {
    let a91 = ldpc::message(c);
    crc::crc14(a91 >> 14) ^ (a91 & 0x3fff) as u16
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
                order3: 0,
            },
        )
        .unwrap();
        assert_eq!(f.codeword, c);
        assert_eq!(f.hard_errors, 29);
    }

    /// The syndrome lookup finds exactly what trying every pair and
    /// triple finds.
    #[test]
    fn lookup_matches_brute_force() {
        let mut seed = 12345u64;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f32 / (1u64 << 53) as f32
        };
        let mut found = 0;
        for trial in 0..40 {
            let m = (trial as u128 * 0x1F35_A7C3_9E21_B44D_0123) & ((1 << 77) - 1);
            let c = ldpc::encode(crc::with_crc(m));
            // Noisy soft bits: around 20 % of them wrong, weakly.
            let llr: [f32; N] = std::array::from_fn(|i| {
                let s = if ldpc::get(&c, i) { 1.0 } else { -1.0 };
                s * (2.0 * rnd() - 0.45) * 4.0
            });
            let fast = decode(
                &llr,
                Depth {
                    order1: 91,
                    order2: 30,
                    order3: 20,
                },
            );
            let slow = brute(&llr, 30, 20);
            assert_eq!(fast.as_ref().map(|f| f.codeword), slow, "trial {trial}");
            found += fast.is_some() as usize;
        }
        assert!(found > 5, "only {found} decodes: test too hard");
    }

    /// Order 0-3 by trying every combination (the same row reduction).
    fn brute(llr: &[f32; N], o2: usize, o3: usize) -> Option<Bits> {
        // Reuse decode's reduction by asking for order 0 only, then redo
        // the search here from the same basis.
        let mut order: [usize; N] = std::array::from_fn(|i| i);
        order.sort_by(|&a, &b| llr[b].abs().total_cmp(&llr[a].abs()));
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
        let mut try_rows = |rows: &[usize]| {
            let mut c = c0;
            for &i in rows {
                xor(&mut c, &g[i]);
            }
            if crc::check(ldpc::message(&c)).is_none_or(|m| m == 0) {
                return;
            }
            let d = distance(&c, &hard, llr);
            if best.is_none_or(|(_, bd)| d < bd) {
                best = Some((c, d));
            }
        };
        try_rows(&[]);
        for a in 0..K {
            try_rows(&[a]);
        }
        for a in K - o2..K {
            for b in a + 1..K {
                try_rows(&[a, b]);
            }
        }
        for a in K - o3..K {
            for b in a + 1..K {
                for c in b + 1..K {
                    try_rows(&[a, b, c]);
                }
            }
        }
        best.map(|(c, _)| c)
    }
}
