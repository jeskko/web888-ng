// SPDX-License-Identifier: GPL-3.0-or-later
//! UTC calendar fields of a Unix time (no time zone database needed).

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Utc {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub min: u32,
    pub sec: u32,
    pub ms: u32,
}

/// The UTC fields of `t` (seconds since the epoch).
pub fn utc(t: f64) -> Utc {
    let secs = t.floor() as i64;
    let ms = ((t - t.floor()) * 1000.0).floor() as u32;
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400) as u32;
    // days to civil (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    Utc {
        year: yoe + era * 400 + i64::from(month <= 2),
        month,
        day,
        hour: sod / 3600,
        min: sod / 60 % 60,
        sec: sod % 60,
        ms: ms.min(999),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        let u = utc(1_792_789_905.25);
        assert_eq!(
            (u.year, u.month, u.day, u.hour, u.min, u.sec, u.ms),
            (2026, 10, 23, 21, 11, 45, 250)
        );
        assert_eq!(utc(951_782_400.0).day, 29);
        assert_eq!(utc(0.0).year, 1970);
    }
}
