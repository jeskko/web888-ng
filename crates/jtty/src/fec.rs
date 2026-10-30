// SPDX-License-Identifier: GPL-3.0-or-later
//! The tiered list decoder (jtty-cuf `receiver_fec.cpp`): coherent blocks
//! of 1, 2 and 4 symbols on the full-symbol observations, then single
//! symbols on the split-symbol magnitudes. Each tier runs a two-lap
//! circular list Viterbi search (four paths per state) over the tail-biting
//! code and tests at most four ranked words; the first CRC-valid one
//! decides.

use crate::protocol::{InfoBits, PAIR_TO_TONE, PayloadBits, crc_valid};
use num_complex::Complex32;

pub(crate) const DATA_SYMBOLS: usize = 46;
const STATES: usize = 512;
const PATHS_PER_STATE: usize = 4;
const LAPS: usize = 2;
const MAX_HYPOTHESES: usize = 4;
const RESERVED_BIT: usize = 32;

pub(crate) type Observation = [[Complex32; 4]; DATA_SYMBOLS];

/// Per data symbol and tone: the full-symbol complex observation and the
/// split-symbol magnitude.
#[derive(Clone)]
pub(crate) struct SoftObservations {
    pub full: Observation,
    pub split: [[f32; 4]; DATA_SYMBOLS],
}

impl Default for SoftObservations {
    fn default() -> Self {
        SoftObservations {
            full: [[Complex32::new(0.0, 0.0); 4]; DATA_SYMBOLS],
            split: [[0.0; 4]; DATA_SYMBOLS],
        }
    }
}

pub(crate) struct FecOutcome {
    pub payload: Option<PayloadBits>,
    pub crc_hypotheses: usize,
}

fn tone_for(state: usize, bit: usize) -> usize {
    let reg = (state << 1) | bit;
    let first = (reg & 0o1167).count_ones() & 1;
    let second = (reg & 0o1545).count_ones() & 1;
    PAIR_TO_TONE[((first << 1) | second) as usize] as usize
}

fn advance(state: usize, bit: usize) -> usize {
    ((state << 1) | bit) & (STATES - 1)
}

#[derive(Clone, Copy)]
struct Block {
    first: usize,
    length: usize,
}

fn blocks_for(block_length: usize) -> Vec<Block> {
    (0..DATA_SYMBOLS)
        .step_by(block_length)
        .map(|first| Block {
            first,
            length: block_length.min(DATA_SYMBOLS - first),
        })
        .collect()
}

/// |sum O_k(m)|^2 / L for every (predecessor, input word) of a block; the
/// word's most significant bit is the block's first bit.
fn branch_table(o: &Observation, block: Block) -> Vec<f32> {
    let words = 1usize << block.length;
    let mut table = vec![0f32; STATES * words];
    for from in 0..STATES {
        for word in 0..words {
            let mut state = from;
            let mut sum = Complex32::new(0.0, 0.0);
            for q in 0..block.length {
                let bit = (word >> (block.length - 1 - q)) & 1;
                sum += o[block.first + q][tone_for(state, bit)];
                state = advance(state, bit);
            }
            table[from * words + word] = sum.norm_sqr() / block.length as f32;
        }
    }
    table
}

#[derive(Clone, Copy, Default)]
struct Path {
    metric: f32,
    origin: u16,
    bits: u64,
}

/// Descending metric, then smaller origin, then smaller bits.
fn path_before(a: &Path, b: &Path) -> bool {
    if a.metric != b.metric {
        return a.metric > b.metric;
    }
    (a.origin, a.bits) < (b.origin, b.bits)
}

#[derive(Clone, Copy, Default)]
struct StatePaths {
    path: [Path; PATHS_PER_STATE],
    count: usize,
}

impl StatePaths {
    fn erase_at(&mut self, i: usize) {
        for k in i + 1..self.count {
            self.path[k - 1] = self.path[k];
        }
        self.count -= 1;
    }

    fn insert(&mut self, candidate: Path, merge: bool) {
        if merge {
            // equal (origin, bits): keep the larger metric
            for i in 0..self.count {
                if self.path[i].origin == candidate.origin && self.path[i].bits == candidate.bits {
                    if !(candidate.metric > self.path[i].metric) {
                        return;
                    }
                    self.erase_at(i);
                    break;
                }
            }
        }
        let mut at = self.count;
        while at > 0 && path_before(&candidate, &self.path[at - 1]) {
            at -= 1;
        }
        if at >= PATHS_PER_STATE {
            return;
        }
        let last = self.count.min(PATHS_PER_STATE - 1);
        let mut k = last;
        while k > at {
            self.path[k] = self.path[k - 1];
            k -= 1;
        }
        self.path[at] = candidate;
        self.count = (self.count + 1).min(PATHS_PER_STATE);
    }
}

struct Word {
    bits: u64,
    accumulated: f32,
    rescore: f32,
    start_state: usize,
}

fn info_from(bits: u64) -> InfoBits {
    let mut info = [0u8; 46];
    for (i, b) in info.iter_mut().enumerate() {
        *b = ((bits >> (DATA_SYMBOLS - 1 - i)) & 1) as u8;
    }
    info
}

/// Ranking value with information bit b as numeric bit b - 1.
fn reversed_value(bits: u64) -> u64 {
    (0..DATA_SYMBOLS).fold(0, |v, i| v | (((bits >> (DATA_SYMBOLS - 1 - i)) & 1) << i))
}

fn list_viterbi(o: &Observation, block_length: usize) -> Vec<Word> {
    let blocks = blocks_for(block_length);
    let tables: Vec<Vec<f32>> = blocks.iter().map(|&b| branch_table(o, b)).collect();
    let mut current = vec![StatePaths::default(); STATES];
    for (s, c) in current.iter_mut().enumerate() {
        c.path[0] = Path {
            metric: 0.0,
            origin: s as u16,
            bits: 0,
        };
        c.count = 1;
    }
    let mut next = vec![StatePaths::default(); STATES];
    for _lap in 0..LAPS {
        for (s, c) in current.iter_mut().enumerate() {
            for p in c.path.iter_mut().take(c.count) {
                p.origin = s as u16;
                p.bits = 0;
            }
        }
        for (j, block) in blocks.iter().enumerate() {
            let words = 1usize << block.length;
            let keep_low = 9 - block.length;
            let reserved_mask = if (block.first..block.first + block.length).contains(&RESERVED_BIT)
            {
                1usize << (block.length - 1 - (RESERVED_BIT - block.first))
            } else {
                0
            };
            let table = &tables[j];
            let merge = j == 0;
            for e in 0..STATES {
                let word = e & (words - 1);
                let kept = &mut next[e];
                kept.count = 0;
                if word & reserved_mask != 0 {
                    continue;
                }
                let low = e >> block.length;
                for high in 0..words {
                    let from = low | (high << keep_low);
                    let branch = table[from * words + word];
                    let src = current[from];
                    for p in &src.path[..src.count] {
                        kept.insert(
                            Path {
                                metric: p.metric + branch,
                                origin: p.origin,
                                bits: (p.bits << block.length) | word as u64,
                            },
                            merge,
                        );
                    }
                }
            }
            std::mem::swap(&mut current, &mut next);
        }
    }
    // lap consistency, the word set
    let mut words: Vec<Word> = Vec::new();
    for (s, c) in current.iter().enumerate() {
        for p in &c.path[..c.count] {
            if p.origin as usize != s {
                continue;
            }
            if let Some(w) = words.iter_mut().find(|w| w.bits == p.bits) {
                if p.metric > w.accumulated || (p.metric == w.accumulated && s < w.start_state) {
                    w.accumulated = p.metric;
                    w.start_state = s;
                }
            } else {
                words.push(Word {
                    bits: p.bits,
                    accumulated: p.metric,
                    rescore: 0.0,
                    start_state: s,
                });
            }
        }
    }
    // circular rescore from each word's own tail-biting start state
    for w in &mut words {
        let start = (w.bits & (STATES as u64 - 1)) as usize;
        w.start_state = start;
        let mut state = start;
        let mut total = 0f32;
        for (j, block) in blocks.iter().enumerate() {
            let mask = (1u64 << block.length) - 1;
            let input = ((w.bits >> (DATA_SYMBOLS - block.first - block.length)) & mask) as usize;
            total += tables[j][state * (1 << block.length) + input];
            for q in 0..block.length {
                state = advance(state, (input >> (block.length - 1 - q)) & 1);
            }
        }
        w.rescore = total;
    }
    words.sort_by(|a, b| {
        b.rescore
            .partial_cmp(&a.rescore)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(reversed_value(a.bits).cmp(&reversed_value(b.bits)))
            .then(a.start_state.cmp(&b.start_state))
            .then(
                b.accumulated
                    .partial_cmp(&a.accumulated)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });
    words.truncate(MAX_HYPOTHESES);
    words
}

pub(crate) fn decode_tiers(o: &SoftObservations) -> FecOutcome {
    let mut outcome = FecOutcome {
        payload: None,
        crc_hypotheses: 0,
    };
    let mut split = [[Complex32::new(0.0, 0.0); 4]; DATA_SYMBOLS];
    for m in 0..DATA_SYMBOLS {
        for k in 0..4 {
            split[m][k] = Complex32::new(o.split[m][k], 0.0);
        }
    }
    let tiers: [(&Observation, usize); 4] = [(&o.full, 1), (&o.full, 2), (&o.full, 4), (&split, 1)];
    for (source, block) in tiers {
        for w in list_viterbi(source, block) {
            outcome.crc_hypotheses += 1;
            let info = info_from(w.bits);
            if !crc_valid(&info) {
                continue;
            }
            let mut payload = [0u8; 34];
            payload.copy_from_slice(&info[..34]);
            if payload.iter().all(|&b| b == 0) {
                break; // the tier rejects; lower-ranked words aren't tried
            }
            outcome.payload = Some(payload);
            return outcome;
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{encode_payload, payload_from_word};

    #[test]
    fn clean_tones_decode_in_the_first_tier() {
        let f = encode_payload(&payload_from_word(0x5A5A_1237, true)).unwrap();
        let mut o = SoftObservations::default();
        for m in 0..DATA_SYMBOLS {
            let t = f.tones[13 + m] as usize;
            o.full[m][t] = Complex32::new(1.0, 0.0);
            o.split[m][t] = 1.0;
        }
        assert_eq!(decode_tiers(&o).payload, Some(f.payload));
    }
}
