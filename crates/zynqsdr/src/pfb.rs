// SPDX-License-Identifier: GPL-3.0-or-later
//! The channelizer bitstream's rings (web888-ng-hw `fpga/doc/register-map.md`,
//! "The channelizer build"), as `ZSDR_PFB_READ` hands them over: 64-bit
//! little-endian words, frames starting with a header
//! `{0x5042, type, sub, 0, number}`.
//!
//! * bins (type 1, sub = the mask's generation, number = frame): then the
//!   selected bins, lowest first, `{Q, I}` with I low (a full-scale sine
//!   reads 2^29, as RX samples).
//! * power (type 2, sub = chunk 0..15, number = line): then 64 words of two
//!   float32 each, `{f32[2j+1], f32[2j]}`: |X|² of the 25-bit bins summed
//!   over the line's frames.
//!
//! A frame is written whole or dropped whole, but a read can start or end
//! inside one, and a reader a ring behind sees old and new data mixed: the
//! parsers resync on headers and check the numbers.

use std::collections::VecDeque;
use std::sync::Arc;
use web888_radio::Iq32;
use web888_radio::bins::BinFrame;

const MAGIC: u64 = 0x5042;
const T_BINS: u8 = 1;
const T_POWER: u8 = 2;

/// Bins in the hardware's power line (all of them; bin 2047 can't be
/// selected for the bins stream).
pub const POWER_BINS: usize = 2048;
const CHUNKS: usize = 16;
const CHUNK_WORDS: usize = 64;

/// |X|² of a full-scale sine in the power line, per frame: the 25-bit
/// bins read 2^24 for it.
const POWER_FULL_SCALE: f64 = (1u64 << 48) as f64;

/// `(type, sub, number)` of a header word.
fn header(w: u64) -> Option<(u8, u8, u32)> {
    (w >> 48 == MAGIC).then_some((((w >> 44) & 0xf) as u8, ((w >> 40) & 0xf) as u8, w as u32))
}

fn is_header(w: u64, t: u8) -> bool {
    header(w).is_some_and(|(ty, _, _)| ty == t)
}

/// Bytes to words, keeping a partial word for the next call.
#[derive(Default)]
struct Words {
    words: Vec<u64>,
    bytes: Vec<u8>,
}

impl Words {
    fn push(&mut self, data: &[u8]) {
        self.bytes.extend_from_slice(data);
        let whole = self.bytes.len() / 8 * 8;
        self.words.extend(
            self.bytes[..whole]
                .as_chunks::<8>()
                .0
                .iter()
                .map(|c| u64::from_le_bytes(*c)),
        );
        self.bytes.drain(..whole);
    }
}

/// Bins frames from the bins ring.
pub struct BinsParser {
    w: Words,
    /// The selection each generation stands for (0: none yet).
    sel: [Arc<[usize]>; 16],
    last: Option<u32>,
    high: u64,
    /// Frames missing between two read in order.
    pub gaps: u64,
    /// Words skipped to find a header.
    pub skipped: u64,
}

impl Default for BinsParser {
    fn default() -> Self {
        BinsParser {
            w: Words::default(),
            sel: std::array::from_fn(|_| Arc::from(Vec::new())),
            last: None,
            high: 0,
            gaps: 0,
            skipped: 0,
        }
    }
}

impl BinsParser {
    /// Frames with `generation` carry `bins` (ascending, as selected).
    pub fn set_selection(&mut self, generation: u8, bins: Arc<[usize]>) {
        self.sel[(generation & 0xf) as usize] = bins;
    }

    /// Parse `data`; append the whole frames to `out`, return how many.
    pub fn push(&mut self, data: &[u8], out: &mut Vec<BinFrame>) -> usize {
        self.w.push(data);
        let w = &self.w.words;
        let mut p = 0;
        let mut n = 0;
        loop {
            while p < w.len() && !is_header(w[p], T_BINS) {
                p += 1;
                self.skipped += 1;
            }
            let Some((_, generation, num)) = w.get(p).and_then(|&h| header(h)) else {
                break;
            };
            let bins = &self.sel[generation as usize];
            let end = p + 1 + bins.len();
            if end > w.len() {
                break; // the rest of the frame comes later
            }
            // a whole frame is followed by the next header: otherwise this
            // wasn't one (a lap mixed old and new data)
            if end < w.len() && !is_header(w[end], T_BINS) {
                p += 1;
                self.skipped += 1;
                continue;
            }
            if let Some(last) = self.last {
                if num <= last && last - num > u32::MAX / 2 {
                    self.high += 1 << 32;
                }
                if num != last.wrapping_add(1) {
                    self.gaps += 1;
                }
            }
            self.last = Some(num);
            out.push(BinFrame {
                frame: self.high | num as u64,
                generation,
                bins: bins.clone(),
                iq: w[p + 1..end]
                    .iter()
                    .map(|&x| Iq32 {
                        i: x as i32,
                        q: (x >> 32) as i32,
                    })
                    .collect(),
            });
            n += 1;
            p = end;
        }
        self.w.words.drain(..p);
        n
    }
}

/// Power lines from the power ring.
pub struct PowerParser {
    w: Words,
    /// Frames per line, for the scale.
    frames: u16,
    line: Vec<f32>,
    /// The line being assembled and its next chunk.
    cur: Option<(u32, usize)>,
    ready: VecDeque<(u32, Vec<f32>)>,
    /// Lines given up (a chunk missing or out of order).
    pub broken: u64,
}

impl PowerParser {
    /// Lines kept for a reader that falls behind.
    const KEEP: usize = 16;

    pub fn new(frames: u16) -> Self {
        PowerParser {
            w: Words::default(),
            frames: frames.max(1),
            line: vec![0.0; POWER_BINS],
            cur: None,
            ready: VecDeque::new(),
            broken: 0,
        }
    }

    pub fn push(&mut self, data: &[u8]) {
        self.w.push(data);
        let w = &self.w.words;
        let scale = 1.0 / (POWER_FULL_SCALE * self.frames as f64);
        let mut p = 0;
        loop {
            while p < w.len() && !is_header(w[p], T_POWER) {
                p += 1;
            }
            let Some((_, chunk, line)) = w.get(p).and_then(|&h| header(h)) else {
                break;
            };
            if p + 1 + CHUNK_WORDS > w.len() {
                break;
            }
            let chunk = chunk as usize;
            match self.cur {
                Some((l, next)) if l == line && next == chunk => {}
                _ if chunk == 0 => {
                    if self.cur.is_some() {
                        self.broken += 1;
                    }
                }
                _ => {
                    // not the chunk this line needs: wait for a line start
                    if self.cur.take().is_some() {
                        self.broken += 1;
                    }
                    p += 1 + CHUNK_WORDS;
                    continue;
                }
            }
            for (j, &x) in w[p + 1..p + 1 + CHUNK_WORDS].iter().enumerate() {
                let k = chunk * 2 * CHUNK_WORDS + 2 * j;
                self.line[k] = (f32::from_bits(x as u32) as f64 * scale) as f32;
                self.line[k + 1] = (f32::from_bits((x >> 32) as u32) as f64 * scale) as f32;
            }
            if chunk == CHUNKS - 1 {
                self.cur = None;
                if self.ready.len() == Self::KEEP {
                    self.ready.pop_front();
                }
                self.ready.push_back((line, self.line.clone()));
            } else {
                self.cur = Some((line, chunk + 1));
            }
            p += 1 + CHUNK_WORDS;
        }
        self.w.words.drain(..p);
    }

    /// The oldest complete line not yet taken.
    pub fn pop(&mut self) -> Option<(u32, Vec<f32>)> {
        self.ready.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hdr(t: u64, sub: u64, num: u32) -> u64 {
        MAGIC << 48 | t << 44 | sub << 40 | num as u64
    }

    fn bytes(words: &[u64]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    fn bin(i: i32, q: i32) -> u64 {
        (q as u32 as u64) << 32 | i as u32 as u64
    }

    #[test]
    fn bins_frames_across_reads_and_generations() {
        let mut p = BinsParser::default();
        p.set_selection(1, Arc::from(vec![5, 9]));
        p.set_selection(2, Arc::from(vec![7]));
        let mut w = vec![bin(1, 1), bin(2, 2)]; // the end of an older frame
        w.extend([hdr(1, 1, 10), bin(100, -100), bin(3, 4)]);
        w.extend([hdr(1, 1, 11), bin(101, -101), bin(5, 6)]);
        w.extend([hdr(1, 2, 12), bin(-7, 7)]);
        let b = bytes(&w);
        let mut out = vec![];
        // split mid-word and mid-frame
        assert_eq!(p.push(&b[..45], &mut out), 1);
        assert_eq!(p.push(&b[45..], &mut out), 2);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].frame, 10);
        assert_eq!(out[0].generation, 1);
        assert_eq!(&*out[0].bins, &[5, 9]);
        assert_eq!(
            out[0].iq,
            vec![Iq32 { i: 100, q: -100 }, Iq32 { i: 3, q: 4 }]
        );
        assert_eq!(
            (out[2].frame, out[2].generation, &*out[2].bins),
            (12, 2, &[7][..])
        );
        assert_eq!(out[2].iq, vec![Iq32 { i: -7, q: 7 }]);
        assert_eq!((p.gaps, p.skipped), (0, 2));
    }

    #[test]
    fn bins_gaps_wraps_and_a_false_header() {
        let mut p = BinsParser::default();
        p.set_selection(1, Arc::from(vec![3]));
        let mut out = vec![];
        let w = [
            hdr(1, 1, u32::MAX - 1),
            bin(1, 0),
            hdr(1, 1, u32::MAX),
            bin(2, 0),
            hdr(1, 1, 0),
            bin(3, 0),
            hdr(1, 1, 5), // frames 1..4 lost
            bin(4, 0),
            hdr(1, 1, 6), // not followed by a header: a lap's leftover
            bin(5, 0),
            bin(6, 0),
            hdr(1, 1, 7),
            bin(7, 0),
        ];
        p.push(&bytes(&w), &mut out);
        let frames: Vec<u64> = out.iter().map(|f| f.frame).collect();
        assert_eq!(
            frames,
            [
                u32::MAX as u64 - 1,
                u32::MAX as u64,
                1 << 32,
                (1 << 32) + 5,
                (1 << 32) + 7
            ]
        );
        assert_eq!(p.gaps, 2);
    }

    fn power_line(line: u32, value: impl Fn(usize) -> f32) -> Vec<u64> {
        let mut w = vec![];
        for c in 0..CHUNKS as u64 {
            w.push(hdr(2, c, line));
            for j in 0..CHUNK_WORDS {
                let k = c as usize * 128 + 2 * j;
                w.push((value(k + 1).to_bits() as u64) << 32 | value(k).to_bits() as u64);
            }
        }
        w
    }

    #[test]
    fn power_lines_scaled_and_checked() {
        let mut p = PowerParser::new(20);
        // a full-scale sine in bin 100 over 20 frames
        let fs = POWER_FULL_SCALE as f32 * 20.0;
        let mut w = power_line(7, |k| if k == 100 { fs } else { k as f32 });
        let mut broken = power_line(8, |_| 1.0);
        broken.drain(65 * 3..65 * 4); // chunk 3 lost
        w.extend(broken);
        w.extend(power_line(9, |_| 0.0));
        let b = bytes(&w);
        p.push(&b[..1000]);
        p.push(&b[1000..]);
        let (n, l) = p.pop().unwrap();
        assert_eq!(n, 7);
        assert_eq!(l.len(), POWER_BINS);
        assert!((l[100] - 1.0).abs() < 1e-6);
        let want = 2047.0 / (POWER_FULL_SCALE * 20.0);
        assert!((l[2047] as f64 / want - 1.0).abs() < 1e-6, "{}", l[2047]);
        assert_eq!(p.pop().map(|(n, _)| n), Some(9));
        assert!(p.pop().is_none());
        assert_eq!(p.broken, 1);
    }
}
