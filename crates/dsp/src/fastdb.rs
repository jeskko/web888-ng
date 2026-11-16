// SPDX-License-Identifier: GPL-3.0-or-later
//! Power to dB and back without libm: on the A9 `log10f` and `powf` are
//! library calls (about 4 % of both cores for 8 waterfalls on the unit).
//! Exponent from the float's bits, the mantissa by a degree-4 polynomial
//! (least squares over the octave): within 0.001 dB both ways, against the
//! waterfall's 0.63 dB step.

/// 10 log10(2). The constants are kept as computed, beyond f32.
#[allow(clippy::excessive_precision)]
const DB_PER_OCTAVE: f32 = 3.010_299_9;

/// `10 log10(p)`, `p` held to at least 1e-30 (NaN too).
#[inline]
#[allow(clippy::excessive_precision)]
pub fn db10(p: f32) -> f32 {
    let b = p.max(1e-30).to_bits();
    let e = ((b >> 23) & 0xff) as i32 - 127;
    let m = f32::from_bits((b & 0x007f_ffff) | 0x3f80_0000) - 1.0;
    let l2 = 0.000_204_257
        + m * (1.436_097_1 + m * (-0.669_512_5 + m * (0.312_211_6 + m * -0.079_149_58)));
    (e as f32 + l2) * DB_PER_OCTAVE
}

/// `10^(d/10)`, for d within about ±380 dB (beyond: the limits).
#[inline]
#[allow(clippy::excessive_precision)]
pub fn undb10(d: f32) -> f32 {
    let x = (d * (1.0 / DB_PER_OCTAVE)).clamp(-126.0, 127.0);
    // floor without libm
    let mut i = x as i32;
    if i as f32 > x {
        i -= 1;
    }
    let f = x - i as f32;
    let p =
        1.000_007_3 + f * (0.692_931_3 + f * (0.241_710_3 + f * (0.051_666_84 + f * 0.013_676_52)));
    p * f32::from_bits(((i + 127) as u32) << 23)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_to_libm() {
        let mut worst: f32 = 0.0;
        let mut p = 1e-29f32;
        while p < 1e30 {
            let want = 10.0 * p.log10();
            worst = worst.max((db10(p) - want).abs());
            p *= 1.0137;
        }
        assert!(worst < 0.001, "db10 off by {worst} dB");
        assert_eq!(db10(0.0), db10(1e-30));
        assert_eq!(db10(f32::NAN), db10(1e-30));
        let mut worst: f32 = 0.0;
        let mut d = -290.0f32;
        while d < 290.0 {
            let got = undb10(d);
            worst = worst.max((10.0 * (got / 10f32.powf(d / 10.0)).log10()).abs());
            // the round trip
            assert!((db10(got) - d).abs() < 0.002, "{d}");
            d += 0.0731;
        }
        assert!(worst < 0.001, "undb10 off by {worst} dB");
    }
}
