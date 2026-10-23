// SPDX-License-Identifier: GPL-3.0-or-later
//! WSPR's K=32, rate 1/2 convolutional code (Layland-Lushbaugh
//! polynomials): the encoder and the soft-decision Fano sequential
//! decoder.
//!
//! Ported from wsprd's `fano.c` (Copyright 1994 Phil Karn KA9Q, minor
//! modifications by Joe Taylor K1JT), as distributed with WSJT-X's wsprd
//! under GPL-3.0-or-later.

const POLY1: u32 = 0xf2d0_5351;
const POLY2: u32 = 0xe461_3c47;

/// The symbol pair of an encoder state: POLY1's parity in bit 1,
/// POLY2's in bit 0.
fn encode_state(state: u32) -> usize {
    (((state & POLY1).count_ones() & 1) << 1 | ((state & POLY2).count_ones() & 1)) as usize
}

/// Encode `data` (high bit first) into 2 symbols per bit.
pub fn encode(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * 16);
    let mut state = 0u32;
    for &byte in data {
        for i in (0..8).rev() {
            state = state << 1 | ((byte >> i) & 1) as u32;
            let s = encode_state(state);
            out.push((s >> 1) as u8);
            out.push((s & 1) as u8);
        }
    }
    out
}

#[derive(Clone, Copy, Default)]
struct Node {
    encstate: u32,
    gamma: i64,
    metrics: [i32; 4],
    tm: [i32; 2],
    i: usize,
}

/// What a Fano run found.
pub struct Fano {
    /// Decoded bytes (`nbits / 8` of them).
    pub data: Vec<u8>,
    pub metric: i64,
    pub cycles: u32,
    /// False if it timed out.
    pub ok: bool,
}

/// Decode `symbols` (deinterleaved, 0..255 with 128 = erasure, two per
/// bit) into `nbits` bits; `mettab[sent][received]`, threshold step
/// `delta`, at most `maxcycles` per bit.
pub fn fano(
    symbols: &[u8],
    nbits: usize,
    mettab: &[[i32; 256]; 2],
    delta: i64,
    maxcycles: u32,
) -> Fano {
    let mut nodes = vec![Node::default(); nbits + 1];
    let last = nbits - 1;
    let tail = nbits - 31;
    for (k, n) in nodes.iter_mut().take(nbits).enumerate() {
        let (s0, s1) = (symbols[2 * k] as usize, symbols[2 * k + 1] as usize);
        n.metrics = [
            mettab[0][s0] + mettab[0][s1],
            mettab[0][s0] + mettab[1][s1],
            mettab[1][s0] + mettab[0][s1],
            mettab[1][s0] + mettab[1][s1],
        ];
    }
    let mut np = 0usize;
    nodes[0].encstate = 0;
    let lsym = encode_state(nodes[0].encstate);
    let m0 = nodes[0].metrics[lsym];
    // Both polynomials are odd: the 1-branch's symbols complement the
    // 0-branch's.
    let m1 = nodes[0].metrics[3 ^ lsym];
    if m0 > m1 {
        nodes[0].tm = [m0, m1];
    } else {
        nodes[0].tm = [m1, m0];
        nodes[0].encstate += 1;
    }
    nodes[0].i = 0;
    let maxcycles = maxcycles as u64 * nbits as u64;
    let mut t: i64 = 0;
    nodes[0].gamma = 0;
    let mut i: u64 = 1;
    let mut done = false;
    while i <= maxcycles {
        // Look forward
        let ngamma = nodes[np].gamma + nodes[np].tm[nodes[np].i] as i64;
        if ngamma >= t {
            if nodes[np].gamma < t + delta {
                // First visit: tighten the threshold
                while ngamma >= t + delta {
                    t += delta;
                }
            }
            nodes[np + 1].gamma = ngamma;
            nodes[np + 1].encstate = nodes[np].encstate << 1;
            np += 1;
            if np == last + 1 {
                done = true;
                break;
            }
            let n = &mut nodes[np];
            let lsym = encode_state(n.encstate);
            if np >= tail {
                // the tail is all zeroes
                n.tm[0] = n.metrics[lsym];
            } else {
                let m0 = n.metrics[lsym];
                let m1 = n.metrics[3 ^ lsym];
                if m0 > m1 {
                    n.tm = [m0, m1];
                } else {
                    n.tm = [m1, m0];
                    n.encstate += 1;
                }
            }
            n.i = 0;
            i += 1;
            continue;
        }
        // Threshold violated: look backward
        loop {
            if np == 0 || nodes[np - 1].gamma < t {
                // Can't back up either: relax the threshold and look
                // forward again on the better branch.
                t -= delta;
                if nodes[np].i != 0 {
                    nodes[np].i = 0;
                    nodes[np].encstate ^= 1;
                }
                break;
            }
            np -= 1;
            if np < tail && nodes[np].i != 1 {
                nodes[np].i += 1;
                nodes[np].encstate ^= 1;
                break;
            }
        }
        i += 1;
    }
    let metric = nodes[np.min(nbits)].gamma;
    let data = (0..nbits / 8)
        .map(|b| nodes[7 + 8 * b].encstate as u8)
        .collect();
    Fano {
        data,
        metric,
        cycles: (i + 1) as u32,
        ok: done && i < maxcycles,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mettab() -> [[i32; 256]; 2] {
        let m = |k: usize| (10.0 * (crate::tables::METRIC[k] as f64 - 0.45)).round() as i32;
        [std::array::from_fn(m), std::array::from_fn(|i| m(255 - i))]
    }

    #[test]
    fn decodes_what_it_encodes_with_errors() {
        let data = [0x12u8, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xc0, 0, 0, 0, 0];
        let bits = encode(&data);
        assert_eq!(bits.len(), 176);
        // confident soft symbols, with a few flipped
        let mut s: Vec<u8> = bits[..162]
            .iter()
            .map(|&b| if b == 1 { 230 } else { 25 })
            .collect();
        for k in [3, 40, 77, 120] {
            s[k] = 255 - s[k];
        }
        let f = fano(&s, 81, &mettab(), 60, 10_000);
        assert!(f.ok);
        assert_eq!(&f.data[..7], &data[..7]);
    }
}
