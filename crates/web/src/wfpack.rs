// SPDX-License-Identifier: GPL-3.0-or-later
//! Waterfall lines packed for the web UI (`doc/m3-multi-user.md` §8): each
//! line on its own (no state between lines, so gaps, new views and
//! time-shared views need nothing), as the difference of each level from
//! the one before it in frequency, Rice-coded with the parameter chosen
//! per block of 64 bins. On the remote unit's 40 m view (1024 bins) a line
//! takes 55 % of its bytes, smaller than DEFLATE (58 %) at a small part
//! of the cost (`/data/iq/wfcomp`). A line that wouldn't shrink is sent as
//! it is. The client asks with `wf`'s `pack`; a packed line has
//! `FLAG_PACKED`.
//!
//! Format, bits most significant first: per block of `BLOCK` bins (the
//! last one shorter), k in 3 bits, then per bin the zig-zagged difference
//! u (the first bin's from 128) as u >> k ones, a zero, and u's low k
//! bits. Padded with zeros to a byte.

/// Bins per Rice parameter.
pub const BLOCK: usize = 64;

/// The zig-zagged differences of a line (0, 1, 2, ... for 0, -1, 1, ...).
fn diffs(levels: &[u8], out: &mut Vec<u16>) {
    out.clear();
    let mut prev = 128i16;
    for &v in levels {
        let d = v as i16 - prev;
        out.push(if d >= 0 {
            2 * d as u16
        } else {
            (-2 * d - 1) as u16
        });
        prev = v as i16;
    }
}

struct Bits<'a> {
    out: &'a mut Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits<'_> {
    fn put(&mut self, v: u64, bits: u32) {
        self.acc = (self.acc << bits) | (v & ((1 << bits) - 1));
        self.n += bits;
        while self.n >= 8 {
            self.n -= 8;
            self.out.push((self.acc >> self.n) as u8);
        }
    }

    fn ones(&mut self, mut q: u32) {
        while q >= 32 {
            self.put(u32::MAX as u64, 32);
            q -= 32;
        }
        self.put((1u64 << q) - 1, q);
    }

    fn finish(mut self) {
        if self.n > 0 {
            let pad = 8 - self.n;
            self.put(0, pad);
        }
    }
}

#[derive(Default)]
pub struct WfPack {
    u: Vec<u16>,
    out: Vec<u8>,
}

impl WfPack {
    /// The packed line, or None when it wouldn't be smaller.
    pub fn pack(&mut self, levels: &[u8]) -> Option<&[u8]> {
        diffs(levels, &mut self.u);
        self.out.clear();
        let mut w = Bits {
            out: &mut self.out,
            acc: 0,
            n: 0,
        };
        for block in self.u.chunks(BLOCK) {
            // the best k is near log2 of the mean: try it and its neighbours
            let mean = block.iter().map(|&u| u as u32).sum::<u32>() / block.len() as u32;
            let k0 = 31 - (mean | 1).leading_zeros();
            let k = (k0.saturating_sub(1)..=(k0 + 1).min(7))
                .min_by_key(|&k| block.iter().map(|&u| (u >> k) as u32 + 1 + k).sum::<u32>())
                .unwrap();
            w.put(k as u64, 3);
            for &u in block {
                w.ones((u >> k) as u32);
                w.put(0, 1);
                if k > 0 {
                    w.put(u as u64, k);
                }
            }
            if w.out.len() >= levels.len() {
                return None;
            }
        }
        w.finish();
        (self.out.len() < levels.len()).then_some(&self.out[..])
    }
}

/// The `n` levels of a packed line (the UI's `unpackWf` is this).
pub fn unpack(packed: &[u8], n: usize) -> Option<Vec<u8>> {
    let mut pos = 0usize; // bit position
    let bit = |pos: &mut usize| -> Option<u32> {
        let b = *packed.get(*pos / 8)?;
        let v = (b >> (7 - *pos % 8)) & 1;
        *pos += 1;
        Some(v as u32)
    };
    let mut out = Vec::with_capacity(n);
    let mut prev = 128i16;
    while out.len() < n {
        let mut k = 0;
        for _ in 0..3 {
            k = (k << 1) | bit(&mut pos)?;
        }
        for _ in 0..BLOCK.min(n - out.len()) {
            let mut q = 0u32;
            while bit(&mut pos)? == 1 {
                q += 1;
                if q > 600 {
                    return None;
                }
            }
            let mut u = q << k;
            for i in (0..k).rev() {
                u |= bit(&mut pos)? << i;
            }
            let d = if u & 1 == 0 {
                (u / 2) as i16
            } else {
                -((u as i16 + 1) / 2)
            };
            let v = prev + d;
            if !(0..=255).contains(&v) {
                return None;
            }
            out.push(v as u8);
            prev = v;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise() -> impl FnMut() -> u8 {
        let mut rng = 0x9e37_79b9_7f4a_7c15u64;
        move || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng >> 56) as u8
        }
    }

    /// A noisy floor with a few signals: well under its size, exact back;
    /// a flat line to about a bit a bin; extreme jumps (0 <-> 255) still
    /// round-trip or go unpacked.
    #[test]
    fn lines_round_trip_and_shrink() {
        let mut r = noise();
        let mut p = WfPack::default();
        for line in 0..5 {
            let lv: Vec<u8> = (0..1000)
                .map(|k| {
                    let s = if (k + line) % 200 < 3 { 60 } else { 0 };
                    100 + r() / 32 + s
                })
                .collect();
            let packed = p.pack(&lv).expect("shrinks").to_vec();
            assert!(packed.len() < lv.len() * 6 / 10, "{}", packed.len());
            assert_eq!(unpack(&packed, lv.len()).unwrap(), lv);
        }
        let flat = vec![120u8; 1024];
        let packed = p.pack(&flat).unwrap().to_vec();
        assert!(packed.len() < 140, "{}", packed.len());
        assert_eq!(unpack(&packed, 1024).unwrap(), flat);
        for lv in [
            (0..1024)
                .map(|k| if k % 2 == 0 { 0 } else { 255 })
                .collect::<Vec<u8>>(),
            (0..1024).map(|_| r()).collect(),
        ] {
            if let Some(packed) = p.pack(&lv).map(<[u8]>::to_vec) {
                assert_eq!(unpack(&packed, lv.len()).unwrap(), lv);
            }
        }
        // a truncated line doesn't decode
        let lv: Vec<u8> = (0..256).map(|_| 100 + r() / 32).collect();
        let packed = p.pack(&lv).unwrap().to_vec();
        assert!(unpack(&packed[..packed.len() / 2], 256).is_none());
    }

    /// `tests/proto/wf-packed.json` for the UI's unpacking test: lines and
    /// their packed bytes. `UPDATE_FIXTURES=1` rewrites it.
    #[test]
    fn fixture_is_current() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/proto/wf-packed.json");
        let mut r = noise();
        let mut pk = WfPack::default();
        let hex = |b: &[u8]| b.iter().map(|v| format!("{v:02x}")).collect::<String>();
        let lines: Vec<Vec<u8>> = vec![
            (0..1024)
                .map(|k| 90 + r() / 32 + if k % 300 < 4 { 70 } else { 0 })
                .collect(),
            vec![120; 256],
            (0..300).map(|k| (k % 256) as u8).collect(),
        ];
        let mut want = String::from(
            "{\"about\": \"waterfall lines and their packing (crates/web/src/wfpack.rs)\",\n \"lines\": [\n",
        );
        for (i, lv) in lines.iter().enumerate() {
            let packed = pk.pack(lv).expect("shrinks").to_vec();
            assert_eq!(unpack(&packed, lv.len()).unwrap(), *lv);
            want += &format!(
                "  {{\"levels\": \"{}\", \"packed\": \"{}\"}}{}\n",
                hex(lv),
                hex(&packed),
                if i + 1 < lines.len() { "," } else { "" }
            );
        }
        want += " ]}\n";
        if std::env::var_os("UPDATE_FIXTURES").is_some() {
            std::fs::write(&p, &want).unwrap();
        }
        let have = std::fs::read_to_string(&p).unwrap_or_default();
        assert!(
            have == want,
            "{} is stale: UPDATE_FIXTURES=1 cargo test -p web888-web wfpack",
            p.display()
        );
    }

    /// The ratio and cost on a capture (`/data/iq/wfcomp/wfcap.ts`: u32
    /// length + frame):
    /// `WF_CAPTURE=FILE cargo test --release -p web888-web wfpack -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn ratio_on_a_capture() {
        let b = std::fs::read(std::env::var("WF_CAPTURE").unwrap()).unwrap();
        let (mut i, mut raw, mut packed, mut lines) = (0, 0, 0, 0);
        let mut p = WfPack::default();
        let t = std::time::Instant::now();
        while i + 4 <= b.len() {
            let n = u32::from_le_bytes(b[i..i + 4].try_into().unwrap()) as usize;
            let lv = &b[i + 4 + crate::proto::WF_HEADER..i + 4 + n];
            raw += n;
            let pk = p.pack(lv).map(<[u8]>::to_vec);
            packed += crate::proto::WF_HEADER + pk.as_ref().map_or(lv.len(), Vec::len);
            if let Some(pk) = pk {
                assert_eq!(unpack(&pk, lv.len()).unwrap(), lv);
            }
            lines += 1;
            i += 4 + n;
        }
        let checked = t.elapsed();
        // packing alone
        let t = std::time::Instant::now();
        let mut i = 0;
        while i + 4 <= b.len() {
            let n = u32::from_le_bytes(b[i..i + 4].try_into().unwrap()) as usize;
            std::hint::black_box(p.pack(&b[i + 4 + crate::proto::WF_HEADER..i + 4 + n]));
            i += 4 + n;
        }
        println!(
            "{lines} lines: {:.1} % of the bytes; packing {:.1} us a line ({:.1} with the check)",
            100.0 * packed as f64 / raw as f64,
            t.elapsed().as_secs_f64() * 1e6 / lines as f64,
            checked.as_secs_f64() * 1e6 / lines as f64
        );
    }
}
