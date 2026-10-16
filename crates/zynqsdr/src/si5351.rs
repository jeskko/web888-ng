// SPDX-License-Identifier: GPL-3.0-or-later
//! Si5351 ADC clock (CLK0) programming, from the public datasheet / AN619.
//!
//! The stock FSBL programs CLK0 at boot. Re-programming it is opt-in
//! (`web888d serve --si5351-adc HZ`) and touches only PLLA, MultiSynth0 and
//! CLK0: the input source and crystal settings stay as the FSBL left them.
//! Board facts from web888-ng-hw `docs/server-fpga-interface.md`: I2C bus
//! 0, address 0x60, reference 24.576 MHz by default, CLK0 = ADC clock at
//! 8 mA.

use std::io;

pub const ADDR: u16 = 0x60;
pub const REF_HZ: f64 = 24_576_000.0;
const VCO_MIN: f64 = 600e6;
const VCO_MAX: f64 = 900e6;
const C_MAX: u32 = 1_048_575;

/// A divider a + b/c.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frac {
    pub a: u32,
    pub b: u32,
    pub c: u32,
}

impl Frac {
    pub fn value(&self) -> f64 {
        self.a as f64 + self.b as f64 / self.c as f64
    }

    /// AN619 parameters (P1, P2, P3).
    pub fn params(&self) -> (u32, u32, u32) {
        let f = (128 * self.b as u64 / self.c as u64) as u32;
        let p1 = 128 * self.a + f - 512;
        let p2 = 128 * self.b - self.c * f;
        (p1, p2, self.c)
    }

    /// Eight register bytes in the PLL / MultiSynth layout.
    /// `r_div_bits` goes into bits 6:4 of the third byte (MultiSynth only).
    pub fn regs(&self, r_div_bits: u8) -> [u8; 8] {
        let (p1, p2, p3) = self.params();
        [
            (p3 >> 8) as u8,
            p3 as u8,
            ((r_div_bits & 7) << 4) | ((p1 >> 16) & 3) as u8,
            (p1 >> 8) as u8,
            p1 as u8,
            (((p3 >> 16) & 0xf) << 4) as u8 | ((p2 >> 16) & 0xf) as u8,
            (p2 >> 8) as u8,
            p2 as u8,
        ]
    }
}

/// Best rational approximation b/c of `x` in [0, 1) with c <= C_MAX.
fn rational(x: f64) -> (u32, u32) {
    if x < 1e-12 {
        return (0, 1);
    }
    // continued fractions
    let (mut h0, mut h1, mut k0, mut k1) = (0u64, 1u64, 1u64, 0u64);
    let mut v = x;
    let mut best = (0u32, 1u32);
    for _ in 0..64 {
        let a = v.floor() as u64;
        let (h2, k2) = (a * h1 + h0, a * k1 + k0);
        if k2 > C_MAX as u64 {
            break;
        }
        best = (h2 as u32, k2 as u32);
        (h0, h1, k0, k1) = (h1, h2, k1, k2);
        let f = v - a as f64;
        if f < 1e-12 {
            break;
        }
        v = 1.0 / f;
    }
    best
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    pub pll: Frac,
    pub ms: Frac,
    pub out_hz: f64,
}

/// Frequency plan for `f_out`: an even integer MultiSynth (lowest jitter)
/// with the PLL in range, preferring an integer PLL ratio.
pub fn plan(f_out: f64, f_ref: f64) -> Option<Plan> {
    let mut frac_plan = None;
    for d in (6..=1800).step_by(2) {
        let vco = f_out * d as f64;
        if vco < VCO_MIN {
            continue;
        }
        if vco > VCO_MAX {
            break;
        }
        let ratio = vco / f_ref;
        let a = ratio.floor() as u32;
        if !(15..=90).contains(&a) {
            continue;
        }
        let (b, c) = rational(ratio - a as f64);
        let pll = Frac { a, b, c };
        let ms = Frac { a: d, b: 0, c: 1 };
        let p = Plan {
            pll,
            ms,
            out_hz: f_ref * pll.value() / d as f64,
        };
        if b == 0 {
            return Some(p);
        }
        frac_plan.get_or_insert(p);
    }
    frac_plan
}

/// Register writes (address, value) for CLK0 = `p` from PLLA.
pub fn clk0_writes(p: &Plan) -> Vec<(u8, u8)> {
    let mut w = Vec::new();
    w.push((3, 0xff)); // disable all outputs while changing
    for (k, v) in p.pll.regs(0).iter().enumerate() {
        w.push((26 + k as u8, *v)); // PLLA (MSNA)
    }
    for (k, v) in p.ms.regs(0).iter().enumerate() {
        w.push((42 + k as u8, *v)); // MultiSynth0, R0 = 1
    }
    // CLK0: powered, integer mode, from PLLA, not inverted, MS0, 8 mA
    let int = (p.ms.b == 0 && p.ms.a.is_multiple_of(2)) as u8;
    w.push((16, (int << 6) | (0b11 << 2) | 0b11));
    w.push((177, 0x20)); // reset PLLA
    w.push((3, 0xfe)); // enable CLK0 only
    w
}

/// Write the plan over Linux i2c-dev (`/dev/i2c-0`).
pub fn program(bus: &str, p: &Plan) -> io::Result<()> {
    use std::io::Write;
    use std::os::fd::AsRawFd;
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(bus)?;
    const I2C_SLAVE: libc::c_ulong = 0x0703;
    // SAFETY: plain ioctl on an i2c-dev fd with an integer argument.
    if unsafe { libc::ioctl(f.as_raw_fd(), I2C_SLAVE as _, ADDR as libc::c_ulong) } < 0 {
        return Err(io::Error::last_os_error());
    }
    for (reg, val) in clk0_writes(p) {
        f.write_all(&[reg, val])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_adc_clocks_are_integer_plans() {
        for (f, pll_a, ms) in [
            (122_880_000.0, 30, 6), // HF: VCO 737.28 MHz
            (61_440_000.0, 25, 10), // narrowband: VCO 614.4 MHz
            (98_304_000.0, 32, 8),  // airband profile 0
            (110_592_000.0, 27, 6), // airband profile 1
        ] {
            let p = plan(f, REF_HZ).unwrap();
            assert_eq!(
                p.pll,
                Frac {
                    a: pll_a,
                    b: 0,
                    c: 1
                },
                "{f}"
            );
            assert_eq!(p.ms, Frac { a: ms, b: 0, c: 1 }, "{f}");
            assert_eq!(p.out_hz, f);
        }
    }

    #[test]
    fn fractional_plan_is_accurate() {
        let f = 122_880_000.0 * (1.0 + 3.3e-6); // GPS-trimmed
        let p = plan(f, REF_HZ).unwrap();
        assert!(p.pll.b != 0 && p.pll.c <= C_MAX);
        assert!(((p.out_hz - f) / f).abs() < 1e-11, "{}", p.out_hz - f);
        assert!(plan(1e6, REF_HZ).is_some()); // large even divider
    }

    #[test]
    fn an619_register_encoding() {
        // integer 30: P1 = 128*30 - 512 = 3328, P2 = 0, P3 = 1
        let r = Frac { a: 30, b: 0, c: 1 }.regs(0);
        assert_eq!(r, [0x00, 0x01, 0x00, 0x0d, 0x00, 0x00, 0x00, 0x00]);
        // fractional 32 + 1/3: f = floor(128/3) = 42
        // P1 = 4096 + 42 - 512 = 3626, P2 = 128 - 126 = 2, P3 = 3
        let r = Frac { a: 32, b: 1, c: 3 }.regs(0);
        assert_eq!(r, [0x00, 0x03, 0x00, 0x0e, 0x2a, 0x00, 0x00, 0x02]);
        let w = clk0_writes(&plan(122_880_000.0, REF_HZ).unwrap());
        assert_eq!(w.first(), Some(&(3, 0xff)));
        assert!(w.contains(&(16, 0x4f)));
        assert_eq!(w.last(), Some(&(3, 0xfe)));
    }
}
