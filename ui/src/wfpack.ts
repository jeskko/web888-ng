// SPDX-License-Identifier: GPL-3.0-or-later
// Packed waterfall lines (server crates/web/src/wfpack.rs, its `unpack`):
// per block of 64 bins a 3-bit Rice parameter k, then per bin the
// zig-zagged difference from the previous level (the first's from 128)
// as q ones, a zero and k low bits, most significant bit first.

const BLOCK = 64;

/** The `n` levels of a packed line, or null if it doesn't decode. */
export function unpackWf(p: Uint8Array, n: number): Uint8Array | null {
  const out = new Uint8Array(n);
  const end = 8 * p.length;
  let pos = 0;
  const bit = (): number => {
    if (pos >= end) throw RangeError();
    const v = (p[pos >> 3] >> (7 - (pos & 7))) & 1;
    pos++;
    return v;
  };
  try {
    let prev = 128;
    let i = 0;
    while (i < n) {
      const k = (bit() << 2) | (bit() << 1) | bit();
      for (const stop = Math.min(n, i + BLOCK); i < stop; i++) {
        let q = 0;
        while (bit() === 1) if (++q > 600) return null;
        let u = q << k;
        for (let b = k - 1; b >= 0; b--) u |= bit() << b;
        const v = prev + (u & 1 ? -((u + 1) >> 1) : u >> 1);
        if (v < 0 || v > 255) return null;
        out[i] = v;
        prev = v;
      }
    }
    return out;
  } catch {
    return null;
  }
}
